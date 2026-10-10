//! Print packs: the synthetic entries that a JS or d.ts print reads, copied
//! from a checker thread to its twin (`program::send_dts_twin_job`).
//!
//! PORT: not in Go (perf). Go prints on any goroutine, because factory nodes
//! are shared pointers. Here the checker keeps its nodes (a copy, not a
//! move), so no checker state that holds a node can read a freed one. The
//! twin puts each entry at its index: the two threads take chunk numbers
//! from one counter (`share_synthetic_chunks`), so a handle names the same
//! node on both. A read on the twin of an entry that no pack copied panics
//! (`Slot::Absent`, or a missing chunk), so a node that the walk misses
//! stops the run and never prints a wrong value.

use super::*;

/// A copy of the synthetic entries that some roots reach on one thread
/// (`export_print_pack`). It owns its data, so it can move to another
/// thread.
pub struct PrintPack {
    /// Node and alias slots, by slot index.
    slots: Vec<(u32, Slot)>,
    /// The current astdata node of each node slot, by data index.
    datas: Vec<(u32, crate::astdata::Node)>,
}

impl PrintPack {
    /// The number of slots in the pack.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }
}

/// Copies the synthetic slots of this thread that `roots` reach, with the
/// astdata node of each node slot. From a node slot the walk goes to the
/// children in its data, its parent and the nodes of its SourceFile fields,
/// and to the nodes that `more` adds for it (the emit context entries of
/// the node). A parsed node ends the walk: every thread reads it. `more`
/// must not make or change synthetic nodes.
pub fn export_print_pack(roots: &[Node], mut more: impl FnMut(Node, &mut Vec<Node>)) -> PrintPack {
    let mut pack = PrintPack {
        slots: Vec::new(),
        datas: Vec::new(),
    };
    let mut seen = SEEN.take();
    let mut stack: Vec<u32> = Vec::new();
    let mut extra: Vec<Node> = Vec::new();
    let push = |stack: &mut Vec<u32>, n: Node| {
        if is_synthetic_node(n) {
            stack.push(slot_index(n) as u32);
        }
    };
    for &root in roots {
        push(&mut stack, root);
    }
    ARENA.with(|a| {
        let a = a.borrow();
        seen.resize(a.slots.len() * SLOT_CHUNK / 64 + 1, 0);
        while let Some(index) = stack.pop() {
            let (word, bit) = (index as usize / 64, 1u64 << (index % 64));
            if seen[word] & bit != 0 {
                continue;
            }
            seen[word] |= bit;
            match a.slot(index as usize) {
                // Slot 0: every twin has it.
                Slot::Nil => {}
                Slot::Alias(target) => pack.slots.push((index, Slot::Alias(*target))),
                Slot::Node(node) => {
                    pack.slots.push((index, Slot::Node(node.clone())));
                    let data = a.data(node.data);
                    pack.datas.push((node.data, data.clone()));
                    data.for_each_child(|child| stack.push(child.index() as u32));
                    push(&mut stack, node.parent);
                    if let Some(file) = &node.source_file {
                        for &n in file.imports.iter().chain(&file.module_augmentations) {
                            push(&mut stack, n);
                        }
                        push(&mut stack, file.common_js_module_indicator);
                        push(&mut stack, file.external_module_indicator);
                    }
                    more(handle(index), &mut extra);
                    for n in extra.drain(..) {
                        push(&mut stack, n);
                    }
                }
                Slot::Absent => panic!("{ABSENT}"),
                Slot::Freed => panic!("{PRINT_FREED}"),
            }
        }
    });
    // Clear only the bits of this walk, so the next one starts empty.
    seen[0] &= !1;
    for &(index, _) in &pack.slots {
        seen[index as usize / 64] = 0;
    }
    SEEN.set(seen);
    pack
}

thread_local! {
    /// The slots that `export_print_pack` saw, one bit per slot index. It
    /// is kept between walks, with every bit clear.
    // PERF: a hash set cost about a third of the walk on effect.
    static SEEN: Cell<Vec<u64>> = const { Cell::new(Vec::new()) };
    /// The slot and data chunks that `install_print_pack` made on this
    /// thread since the last `release_print_packs`.
    static INSTALLED: RefCell<(Vec<u32>, Vec<u32>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// Puts the entries of `pack` in this thread's arena, each at its index:
/// a slot replaces the slot there, and a data entry fills its cell unless
/// an earlier pack filled it (a data entry never changes). This thread must
/// be the d.ts twin of the pack's thread (`install_twin_synthetic_arena`):
/// the chunks of the pack's entries are then never chunks of its own.
pub fn install_print_pack(pack: PrintPack) {
    INSTALLED.with_borrow_mut(|(slot_chunks, data_chunks)| {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            debug_assert!(a.shared.is_some(), "a print pack goes to a d.ts twin");
            for (index, slot) in pack.slots {
                let (c, i) = (index as usize / SLOT_CHUNK, index as usize % SLOT_CHUNK);
                if a.slots.get(c).is_none_or(Option::is_none) {
                    set_chunk(&mut a.slots, c as u32, vec![Slot::Absent; SLOT_CHUNK]);
                    if a.slot_owner.len() <= c {
                        a.slot_owner.resize(c + 1, OwnerKey::Base);
                    }
                    slot_chunks.push(c as u32);
                } else if c == NIL_SLOT as usize && slot_chunks.first() != Some(&NIL_SLOT) {
                    // Chunk 0 has the nil slot: `release_print_packs` resets it.
                    slot_chunks.insert(0, NIL_SLOT);
                }
                a.slots[c].as_mut().expect(FREED)[i] = slot;
            }
            for (index, node) in pack.datas {
                let (c, i) = (index as usize / DATA_CHUNK, index as usize % DATA_CHUNK);
                if a.datas.get(c).is_none_or(Option::is_none) {
                    set_chunk(&mut a.datas, c as u32, empty_data_chunk());
                    data_chunks.push(c as u32);
                }
                // A filled cell holds this same node.
                let _ = a.datas[c].as_ref().expect(FREED)[i].set(node);
            }
        });
    });
}

/// Frees the entries that `install_print_pack` put in this thread's arena
/// since the last call: a read of one of them panics again, until a pack
/// brings it back. The d.ts twin calls it after each print, so it keeps the
/// nodes of one print at a time. The entries that this thread made stay.
pub fn release_print_packs() {
    let (slot_chunks, data_chunks) = INSTALLED.take();
    // Dropped after the arena borrow ends.
    let mut freed = Vec::with_capacity(slot_chunks.len());
    let mut freed_data = Vec::with_capacity(data_chunks.len());
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        for c in slot_chunks {
            let chunk = if c == NIL_SLOT {
                let mut nil_chunk = vec![Slot::Absent; SLOT_CHUNK];
                nil_chunk[NIL_SLOT as usize] = Slot::Nil;
                Some(nil_chunk)
            } else {
                None
            };
            freed.push(std::mem::replace(&mut a.slots[c as usize], chunk));
        }
        for c in data_chunks {
            freed_data.push(a.datas[c as usize].take());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // A twin that installs the pack of a tree reads the same tree at the
    // same handles, with the parsed-node aliases resolved. A node that the
    // pack does not hold panics on the twin, and the twin's own new nodes
    // never take a chunk number of the checker.
    #[test]
    fn print_pack_round_trip() {
        std::thread::spawn(|| {
            let f = NodeFactory::new();
            let shared = share_synthetic_chunks();
            let a = f.new_identifier("a");
            let b = f.new_identifier("b");
            let list = f.new_node_list(&[a, b]);
            let block = f.new_block(list, false);
            set_node_parent(a, block);
            let outside = f.new_identifier("outside");
            let pack = export_print_pack(&[block], |_, _| {});
            assert_eq!(pack.slot_count(), 3);

            std::thread::spawn(move || {
                install_twin_synthetic_arena(shared);
                install_print_pack(pack);
                assert_eq!(block.kind(), SyntaxKind::Block);
                assert_eq!(block.statement_list().nodes().to_vec(), vec![a, b]);
                assert_eq!(a.text(), "a");
                assert_eq!(a.parent(), block);
                assert!(std::panic::catch_unwind(|| outside.kind()).is_err());
                let own = NodeFactory::new().new_identifier("own");
                assert!(own != a && own != b && own != block && own != outside);
                assert_eq!(own.text(), "own");

                // A release frees the copies and keeps the twin's own nodes.
                release_print_packs();
                assert!(std::panic::catch_unwind(|| a.kind()).is_err());
                assert_eq!(own.text(), "own");
            })
            .join()
            .expect("twin thread panicked");

            // The checker's next chunks skip the numbers the twin took.
            for i in 0..2 * SLOT_CHUNK {
                let n = f.new_identifier(format!("n{i}"));
                assert_eq!(n.text(), format!("n{i}"));
            }
            assert_eq!(a.text(), "a");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }
}
