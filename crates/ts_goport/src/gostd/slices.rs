//! Go `slices.SortFunc`, `slices.BinarySearchFunc` (go1.27.1
//! `src/slices/sort.go`, `src/slices/zsortanyfunc.go`) and `sort.Slice`
//! (`src/sort/slice.go`, `src/sort/zsortfunc.go`, `src/sort/sort.go`).
//!
//! Go's unstable sorts are pattern-defeating quicksort. The order of equal
//! elements depends on the exact algorithm, so this is a literal port: the
//! resulting permutation equals Go's for every input and comparator.
//!
//! PORT: Go `int` indexes are `isize` here (some Go loops count down past
//! zero); they are cast to `usize` only to index the slice.
//!
//! `sort_func`, `sort_stable_func`, `sort_slice` and `stable_sort_by` have
//! one copy of each algorithm for all element types (`by_index`). Hot sites
//! call `sort_func_hot`, `sort_stable_func_hot` and `stable_sort_by_hot`,
//! which have one copy per element and comparator type.
//!
//! Also the capacity that Go `append` gives a full slice
//! (`go_byte_slice_grow`, `go_ptr_slice_grow`) for ports whose result depends
//! on it.

use crate::prelude::*;

pub use by_index::{sort_func, sort_slice, sort_stable_func, stable_sort_by};
#[cfg(target_family = "wasm")]
pub use by_index::{
    sort_func as sort_func_hot, sort_stable_func as sort_stable_func_hot,
    stable_sort_by as stable_sort_by_hot,
};

/// `stable_sort_by` for hot sites: Rust's stable `x.sort_by(cmp)`, with a
/// copy per element and comparator type. On wasm it is `stable_sort_by`.
#[cfg(not(target_family = "wasm"))]
pub fn stable_sort_by_hot<T, F: FnMut(&T, &T) -> std::cmp::Ordering>(x: &mut [T], cmp: F) {
    x.sort_by(cmp);
}

/// `sort_func`, `sort_stable_func`, `sort_slice` and `stable_sort_by` with
/// one copy of each algorithm for all element types. Each sorts the indexes
/// of `x` (`u32`) with a `dyn` comparator, then puts `x` in that order. The
/// Go algorithms make the same comparisons, in the same order, and the same
/// swaps on the indexes as on `x`, so the result is the same (for
/// `stable_sort_by`, see there). A copy per element and comparator type is
/// about 4 to 10 KB of native code (about 0.6 MB in all) and was 49 KB of
/// the wasm module (123 copies) for the Go sorts, and 50 KB (15 copies) for
/// std's stable sort. A slice of under 2 elements returns at once: no
/// algorithm compares there.
mod by_index {
    use super::{LessSwap, bits_len, pdqsort_cmp_func, pdqsort_func, stable_cmp_func};
    use std::cmp::Ordering;

    /// Go `slices.SortFunc` (see `sort_func_hot`).
    pub fn sort_func<T, F: FnMut(&T, &T) -> i32>(x: &mut [T], mut cmp: F) {
        if x.len() < 2 {
            return;
        }
        let mut order = indexes(x.len());
        sort_indexes_cmp(&mut order, &mut |a, b| {
            cmp(&x[*a as usize], &x[*b as usize])
        });
        put_in_order(&mut order, &mut |i, j| x.swap(i, j));
    }

    /// Go `slices.SortStableFunc` (see `sort_stable_func_hot`).
    pub fn sort_stable_func<T, F: FnMut(&T, &T) -> i32>(x: &mut [T], mut cmp: F) {
        if x.len() < 2 {
            return;
        }
        let mut order = indexes(x.len());
        stable_sort_indexes_cmp(&mut order, &mut |a, b| {
            cmp(&x[*a as usize], &x[*b as usize])
        });
        put_in_order(&mut order, &mut |i, j| x.swap(i, j));
    }

    /// Go `sort.Slice` (`sort/slice.go:24`; the algorithm is `pdqsort_func`).
    pub fn sort_slice<T, F: FnMut(&T, &T) -> bool>(x: &mut [T], mut less: F) {
        if x.len() < 2 {
            return;
        }
        let mut order = indexes(x.len());
        sort_indexes_less(&mut order, &mut |a, b| {
            less(&x[*a as usize], &x[*b as usize])
        });
        put_in_order(&mut order, &mut |i, j| x.swap(i, j));
    }

    /// Rust's stable `x.sort_by(cmp)`, for a `cmp` that is a strict weak
    /// order (for example a compare of plain keys). Use it in place of
    /// `sort`, `sort_by` and `sort_by_key` at cold sites. std's sort makes
    /// other comparisons on the indexes than on `x`. But with a strict weak
    /// order, a stable sort has one correct result, so it is the same.
    pub fn stable_sort_by<T, F: FnMut(&T, &T) -> Ordering>(x: &mut [T], mut cmp: F) {
        if x.len() < 2 {
            return;
        }
        let mut order = indexes(x.len());
        sort_indexes(&mut order, &mut |a, b| {
            cmp(&x[*a as usize], &x[*b as usize])
        });
        put_in_order(&mut order, &mut |i, j| x.swap(i, j));
    }

    // The sorts of `order`, each in one function: without `inline(never)`
    // the callers could inline a copy (std's `sort_by` inlines its insertion
    // sort for short slices into the caller).

    #[inline(never)]
    fn sort_indexes_cmp(order: &mut [u32], mut cmp: &mut dyn FnMut(&u32, &u32) -> i32) {
        let n = order.len() as isize;
        pdqsort_cmp_func(order, 0, n, bits_len(n as usize), &mut cmp);
    }

    #[inline(never)]
    fn stable_sort_indexes_cmp(order: &mut [u32], mut cmp: &mut dyn FnMut(&u32, &u32) -> i32) {
        let n = order.len() as isize;
        stable_cmp_func(order, n, &mut cmp);
    }

    #[inline(never)]
    fn sort_indexes_less(order: &mut [u32], less: &mut dyn FnMut(&u32, &u32) -> bool) {
        let n = order.len() as isize;
        let mut data = LessSwap {
            data: order,
            less_fn: less,
        };
        pdqsort_func(&mut data, 0, n, bits_len(n as usize));
    }

    #[inline(never)]
    fn sort_indexes(order: &mut [u32], cmp: &mut dyn FnMut(&u32, &u32) -> Ordering) {
        order.sort_by(cmp);
    }

    /// The indexes `0..len`, in order.
    #[inline(never)]
    fn indexes(len: usize) -> Vec<u32> {
        (0..u32::try_from(len).expect("a slice of under 4G elements")).collect()
    }

    /// Moves the element at `order[i]` of `x` to `i`, for each `i`, by
    /// following each cycle of `order` with swaps. Leaves `order` as `0..n`.
    /// `swap(i, j)` swaps elements `i` and `j` of `x`.
    #[inline(never)]
    fn put_in_order(order: &mut [u32], swap: &mut dyn FnMut(usize, usize)) {
        for start in 0..order.len() {
            let mut current = start;
            loop {
                let next = order[current] as usize;
                order[current] = current as u32;
                if next == start {
                    break;
                }
                swap(current, next);
                current = next;
            }
        }
    }
}

// Go: slices/sort.go:30 SortFunc
/// SortFunc sorts the slice x in ascending order as determined by the cmp
/// function. This sort is not guaranteed to be stable.
/// cmp(a, b) should return a negative number when a < b, a positive number
/// when a > b and zero when a == b or a and b are incomparable in the sense
/// of a strict weak ordering.
///
/// SortFunc requires that cmp is a strict weak ordering.
/// See <https://en.wikipedia.org/wiki/Weak_ordering#Strict_weak_orderings>.
/// The function should return 0 for incomparable items.
///
/// PORT: `sort_func` (`by_index`) is this sort with one copy for all element
/// types. This one is for hot sites: its copy per element and comparator
/// type saves the index vector and the `dyn` calls.
#[cfg(not(target_family = "wasm"))]
pub fn sort_func_hot<T, F: FnMut(&T, &T) -> i32>(x: &mut [T], mut cmp: F) {
    let n = x.len() as isize;
    pdqsort_cmp_func(x, 0, n, bits_len(n as usize), &mut cmp);
}

// Go: slices/sort.go:37 SortStableFunc
/// SortStableFunc sorts the slice x while keeping the original order of
/// equal elements, using cmp to compare elements in the same way as
/// [SortFunc].
///
/// PORT: for hot sites, as `sort_func_hot`; `sort_stable_func` is the copy
/// for all element types.
#[cfg(not(target_family = "wasm"))]
pub fn sort_stable_func_hot<T, F: FnMut(&T, &T) -> i32>(x: &mut [T], mut cmp: F) {
    let n = x.len() as isize;
    stable_cmp_func(x, n, &mut cmp);
}

// Go: slices/sort.go:152 BinarySearchFunc
/// BinarySearchFunc works like [BinarySearch], but uses a custom comparison
/// function. The slice must be sorted in increasing order, where "increasing"
/// is defined by cmp. cmp should return 0 if the slice element matches
/// the target, a negative number if the slice element precedes the target,
/// or a positive number if the slice element follows the target.
/// cmp must implement the same ordering as the slice, such that if
/// cmp(a, t) < 0 and cmp(b, t) >= 0, then a must precede b in the slice.
pub fn binary_search_func<E, T, F: FnMut(&E, &T) -> i32>(
    x: &[E],
    target: T,
    mut cmp: F,
) -> (usize, bool) {
    let n = x.len();
    // Define cmp(x[-1], target) < 0 and cmp(x[n], target) >= 0 .
    // Invariant: cmp(x[i - 1], target) < 0, cmp(x[j], target) >= 0.
    let (mut i, mut j) = (0usize, n);
    while i < j {
        let h = (i + j) >> 1; // avoid overflow when computing h
        // i ≤ h < j
        if cmp(&x[h], &target) < 0 {
            i = h + 1; // preserves cmp(x[i - 1], target) < 0
        } else {
            j = h; // preserves cmp(x[j], target) >= 0
        }
    }
    // cmp(x[i-1], target) < 0, cmp(x[j], target) >= 0 and j = i => answer is i.
    (i, i < n && cmp(&x[i], &target) == 0)
}

// Go: slices/sort.go:170 sortedHint
/// sortedHint is a hint for pdqsort when choosing the pivot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortedHint {
    UnknownHint,
    IncreasingHint,
    DecreasingHint,
}

// Go: slices/sort.go:179 xorshift
/// xorshift paper: <https://www.jstatsoft.org/article/view/v008i14/xorshift.pdf>
#[derive(Clone, Copy, Debug)]
pub struct Xorshift(pub u64);

impl Xorshift {
    // Go: slices/sort.go:181 Next
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

// Go: slices/sort.go:188 nextPowerOfTwo
pub fn next_power_of_two(length: isize) -> usize {
    1usize << bits_len(length as usize)
}

// PORT: Go `bits.Len(uint(n))`: the minimum number of bits to represent n.
fn bits_len(n: usize) -> isize {
    (usize::BITS - n.leading_zeros()) as isize
}

// ---------------------------------------------------------------------------
// slices/zsortanyfunc.go (generated from sort/gen_sort_variants.go)
// ---------------------------------------------------------------------------

// Go: slices/zsortanyfunc.go:10 insertionSortCmpFunc
/// insertionSortCmpFunc sorts data[a:b] using insertion sort.
pub fn insertion_sort_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    cmp: &mut F,
) {
    let mut i = a + 1;
    while i < b {
        let mut j = i;
        while j > a && (cmp(&data[j as usize], &data[(j - 1) as usize]) < 0) {
            data.swap(j as usize, (j - 1) as usize);
            j -= 1;
        }
        i += 1;
    }
}

// Go: slices/zsortanyfunc.go:20 siftDownCmpFunc
/// siftDownCmpFunc implements the heap property on data[lo:hi].
/// first is an offset into the array where the root of the heap lies.
pub fn sift_down_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    lo: isize,
    hi: isize,
    first: isize,
    cmp: &mut F,
) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            break;
        }
        if child + 1 < hi
            && (cmp(
                &data[(first + child) as usize],
                &data[(first + child + 1) as usize],
            ) < 0)
        {
            child += 1;
        }
        if !(cmp(
            &data[(first + root) as usize],
            &data[(first + child) as usize],
        ) < 0)
        {
            return;
        }
        data.swap((first + root) as usize, (first + child) as usize);
        root = child;
    }
}

// Go: slices/zsortanyfunc.go:38 heapSortCmpFunc
pub fn heap_sort_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    cmp: &mut F,
) {
    let first = a;
    let lo = 0;
    let hi = b - a;

    // Build heap with greatest element at top.
    let mut i = (hi - 1) / 2;
    while i >= 0 {
        sift_down_cmp_func(data, i, hi, first, cmp);
        i -= 1;
    }

    // Pop elements, largest first, into end of data.
    let mut i = hi - 1;
    while i >= 0 {
        data.swap(first as usize, (first + i) as usize);
        sift_down_cmp_func(data, lo, i, first, cmp);
        i -= 1;
    }
}

// Go: slices/zsortanyfunc.go:61 pdqsortCmpFunc
/// pdqsortCmpFunc sorts data[a:b].
/// The algorithm based on pattern-defeating quicksort(pdqsort), but without the optimizations from BlockQuicksort.
/// pdqsort paper: <https://arxiv.org/pdf/2106.05123.pdf>
/// C++ implementation: <https://github.com/orlp/pdqsort>
/// Rust implementation: <https://docs.rs/pdqsort/latest/pdqsort/>
/// limit is the number of allowed bad (very unbalanced) pivots before falling back to heapsort.
pub fn pdqsort_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    mut a: isize,
    mut b: isize,
    mut limit: isize,
    cmp: &mut F,
) {
    const MAX_INSERTION: isize = 12;

    let mut was_balanced = true; // whether the last partitioning was reasonably balanced
    let mut was_partitioned = true; // whether the slice was already partitioned

    loop {
        let length = b - a;

        if length <= MAX_INSERTION {
            insertion_sort_cmp_func(data, a, b, cmp);
            return;
        }

        // Fall back to heapsort if too many bad choices were made.
        if limit == 0 {
            heap_sort_cmp_func(data, a, b, cmp);
            return;
        }

        // If the last partitioning was imbalanced, we need to breaking patterns.
        if !was_balanced {
            break_patterns_cmp_func(data, a, b, cmp);
            limit -= 1;
        }

        let (mut pivot, mut hint) = choose_pivot_cmp_func(data, a, b, cmp);
        if hint == SortedHint::DecreasingHint {
            reverse_range_cmp_func(data, a, b, cmp);
            // The chosen pivot was pivot-a elements after the start of the array.
            // After reversing it is pivot-a elements before the end of the array.
            // The idea came from Rust's implementation.
            pivot = (b - 1) - (pivot - a);
            hint = SortedHint::IncreasingHint;
        }

        // The slice is likely already sorted.
        if was_balanced && was_partitioned && hint == SortedHint::IncreasingHint {
            if partial_insertion_sort_cmp_func(data, a, b, cmp) {
                return;
            }
        }

        // Probably the slice contains many duplicate elements, partition the slice into
        // elements equal to and elements greater than the pivot.
        if a > 0 && !(cmp(&data[(a - 1) as usize], &data[pivot as usize]) < 0) {
            let mid = partition_equal_cmp_func(data, a, b, pivot, cmp);
            a = mid;
            continue;
        }

        let (mid, already_partitioned) = partition_cmp_func(data, a, b, pivot, cmp);
        was_partitioned = already_partitioned;

        let (left_len, right_len) = (mid - a, b - mid);
        let balance_threshold = length / 8;
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort_cmp_func(data, a, mid, limit, cmp);
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort_cmp_func(data, mid + 1, b, limit, cmp);
            b = mid;
        }
    }
}

// Go: slices/zsortanyfunc.go:135 partitionCmpFunc
/// partitionCmpFunc does one quicksort partition.
/// Let p = data[pivot]
/// Moves elements in data[a:b] around, so that data[i]<p and data[j]>=p for i<newpivot and j>newpivot.
/// On return, data[newpivot] = p
pub fn partition_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    pivot: isize,
    cmp: &mut F,
) -> (isize, bool) {
    data.swap(a as usize, pivot as usize);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    while i <= j && (cmp(&data[i as usize], &data[a as usize]) < 0) {
        i += 1;
    }
    while i <= j && !(cmp(&data[j as usize], &data[a as usize]) < 0) {
        j -= 1;
    }
    if i > j {
        data.swap(j as usize, a as usize);
        return (j, true);
    }
    data.swap(i as usize, j as usize);
    i += 1;
    j -= 1;

    loop {
        while i <= j && (cmp(&data[i as usize], &data[a as usize]) < 0) {
            i += 1;
        }
        while i <= j && !(cmp(&data[j as usize], &data[a as usize]) < 0) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
    data.swap(j as usize, a as usize);
    (j, false)
}

// Go: slices/zsortanyfunc.go:173 partitionEqualCmpFunc
/// partitionEqualCmpFunc partitions data[a:b] into elements equal to data[pivot] followed by elements greater than data[pivot].
/// It assumed that data[a:b] does not contain elements smaller than the data[pivot].
pub fn partition_equal_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    pivot: isize,
    cmp: &mut F,
) -> isize {
    data.swap(a as usize, pivot as usize);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    loop {
        while i <= j && !(cmp(&data[a as usize], &data[i as usize]) < 0) {
            i += 1;
        }
        while i <= j && (cmp(&data[a as usize], &data[j as usize]) < 0) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
    i
}

// Go: slices/zsortanyfunc.go:195 partialInsertionSortCmpFunc
/// partialInsertionSortCmpFunc partially sorts a slice, returns true if the slice is sorted at the end.
pub fn partial_insertion_sort_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    cmp: &mut F,
) -> bool {
    const MAX_STEPS: isize = 5; // maximum number of adjacent out-of-order pairs that will get shifted
    const SHORTEST_SHIFTING: isize = 50; // don't shift any elements on short arrays
    let mut i = a + 1;
    let mut j = 0;
    while j < MAX_STEPS {
        while i < b && !(cmp(&data[i as usize], &data[(i - 1) as usize]) < 0) {
            i += 1;
        }

        if i == b {
            return true;
        }

        if b - a < SHORTEST_SHIFTING {
            return false;
        }

        data.swap(i as usize, (i - 1) as usize);

        // Shift the smaller one to the left.
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !(cmp(&data[j as usize], &data[(j - 1) as usize]) < 0) {
                    break;
                }
                data.swap(j as usize, (j - 1) as usize);
                j -= 1;
            }
        }
        // Shift the greater one to the right.
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b {
                if !(cmp(&data[j as usize], &data[(j - 1) as usize]) < 0) {
                    break;
                }
                data.swap(j as usize, (j - 1) as usize);
                j += 1;
            }
        }
        j += 1;
    }
    false
}

// Go: slices/zsortanyfunc.go:240 breakPatternsCmpFunc
/// breakPatternsCmpFunc scatters some elements around in an attempt to break some patterns
/// that might cause imbalanced partitions in quicksort.
pub fn break_patterns_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    cmp: &mut F,
) {
    let length = b - a;
    if length >= 8 {
        let mut random = Xorshift(length as u64);
        let modulus = next_power_of_two(length);

        let mut idx = a + (length / 4) * 2 - 1;
        while idx <= a + (length / 4) * 2 + 1 {
            let mut other = ((random.next() as usize) & (modulus - 1)) as isize;
            if other >= length {
                other -= length;
            }
            data.swap(idx as usize, (a + other) as usize);
            idx += 1;
        }
    }
}

// Go: slices/zsortanyfunc.go:261 choosePivotCmpFunc
/// choosePivotCmpFunc chooses a pivot in data[a:b].
///
/// [0,8): chooses a static pivot.
/// [8,shortestNinther): uses the simple median-of-three method.
/// [shortestNinther,∞): uses the Tukey ninther method.
pub fn choose_pivot_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    cmp: &mut F,
) -> (isize, SortedHint) {
    const SHORTEST_NINTHER: isize = 50;
    const MAX_SWAPS: isize = 4 * 3;

    let l = b - a;

    let mut swaps: isize = 0;
    let mut i = a + l / 4 * 1;
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;

    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            // Tukey ninther method, the idea came from Rust's implementation.
            i = median_adjacent_cmp_func(data, i, &mut swaps, cmp);
            j = median_adjacent_cmp_func(data, j, &mut swaps, cmp);
            k = median_adjacent_cmp_func(data, k, &mut swaps, cmp);
        }
        // Find the median among i, j, k and stores it into j.
        j = median_cmp_func(data, i, j, k, &mut swaps, cmp);
    }

    match swaps {
        0 => (j, SortedHint::IncreasingHint),
        MAX_SWAPS => (j, SortedHint::DecreasingHint),
        _ => (j, SortedHint::UnknownHint),
    }
}

// Go: slices/zsortanyfunc.go:298 order2CmpFunc
/// order2CmpFunc returns x,y where data[x] <= data[y], where x,y=a,b or x,y=b,a.
pub fn order2_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    swaps: &mut isize,
    cmp: &mut F,
) -> (isize, isize) {
    if cmp(&data[b as usize], &data[a as usize]) < 0 {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

// Go: slices/zsortanyfunc.go:307 medianCmpFunc
/// medianCmpFunc returns x where data[x] is the median of data[a],data[b],data[c], where x is a, b, or c.
pub fn median_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    c: isize,
    swaps: &mut isize,
    cmp: &mut F,
) -> isize {
    let (a, b) = order2_cmp_func(data, a, b, swaps, cmp);
    let (b, c) = order2_cmp_func(data, b, c, swaps, cmp);
    let _ = c;
    let (_a, b) = order2_cmp_func(data, a, b, swaps, cmp);
    b
}

// Go: slices/zsortanyfunc.go:315 medianAdjacentCmpFunc
/// medianAdjacentCmpFunc finds the median of data[a - 1], data[a], data[a + 1] and stores the index into a.
pub fn median_adjacent_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    swaps: &mut isize,
    cmp: &mut F,
) -> isize {
    median_cmp_func(data, a - 1, a, a + 1, swaps, cmp)
}

// Go: slices/zsortanyfunc.go:319 reverseRangeCmpFunc
pub fn reverse_range_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    _cmp: &mut F,
) {
    let mut i = a;
    let mut j = b - 1;
    while i < j {
        data.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
}

// Go: slices/zsortanyfunc.go:329 swapRangeCmpFunc
pub fn swap_range_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    b: isize,
    n: isize,
    _cmp: &mut F,
) {
    let mut i = 0;
    while i < n {
        data.swap((a + i) as usize, (b + i) as usize);
        i += 1;
    }
}

// Go: slices/zsortanyfunc.go:335 stableCmpFunc
pub fn stable_cmp_func<T, F: FnMut(&T, &T) -> i32>(data: &mut [T], n: isize, cmp: &mut F) {
    let mut block_size = 20; // must be > 0
    let (mut a, mut b) = (0, block_size);
    while b <= n {
        insertion_sort_cmp_func(data, a, b, cmp);
        a = b;
        b += block_size;
    }
    insertion_sort_cmp_func(data, a, n, cmp);

    while block_size < n {
        a = 0;
        b = 2 * block_size;
        while b <= n {
            sym_merge_cmp_func(data, a, a + block_size, b, cmp);
            a = b;
            b += 2 * block_size;
        }
        let m = a + block_size;
        if m < n {
            sym_merge_cmp_func(data, a, m, n, cmp);
        }
        block_size *= 2;
    }
}

// Go: slices/zsortanyfunc.go:378 symMergeCmpFunc
/// symMergeCmpFunc merges the two sorted subsequences data[a:m] and data[m:b] using
/// the SymMerge algorithm from Pok-Son Kim and Arne Kutzner, "Stable Minimum
/// Storage Merging by Symmetric Comparisons", in Susanne Albers and Tomasz
/// Radzik, editors, Algorithms - ESA 2004, volume 3221 of Lecture Notes in
/// Computer Science, pages 714-723. Springer, 2004.
///
/// Let M = m-a and N = b-n. Wolog M < N.
/// The recursion depth is bound by ceil(log(N+M)).
/// The algorithm needs O(M*log(N/M + 1)) calls to data.Less.
/// The algorithm needs O((M+N)*log(M)) calls to data.Swap.
///
/// The paper gives O((M+N)*log(M)) as the number of assignments assuming a
/// rotation algorithm which uses O(M+N+gcd(M+N)) assignments. The argumentation
/// in the paper carries through for Swap operations, especially as the block
/// swapping rotate uses only O(M+N) Swaps.
///
/// symMerge assumes non-degenerate arguments: a < m && m < b.
/// Having the caller check this condition eliminates many leaf recursion calls,
/// which improves performance.
pub fn sym_merge_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    m: isize,
    b: isize,
    cmp: &mut F,
) {
    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[a] into data[m:b]
    // if data[a:m] only contains one element.
    if m - a == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] >= data[a] for m <= i < b.
        // Exit the search loop with i == b in case no such index exists.
        let mut i = m;
        let mut j = b;
        while i < j {
            let h = (((i + j) as usize) >> 1) as isize;
            if cmp(&data[h as usize], &data[a as usize]) < 0 {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[a] reaches the position before i.
        let mut k = a;
        while k < i - 1 {
            data.swap(k as usize, (k + 1) as usize);
            k += 1;
        }
        return;
    }

    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[m] into data[a:m]
    // if data[m:b] only contains one element.
    if b - m == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] > data[m] for a <= i < m.
        // Exit the search loop with i == m in case no such index exists.
        let mut i = a;
        let mut j = m;
        while i < j {
            let h = (((i + j) as usize) >> 1) as isize;
            if !(cmp(&data[m as usize], &data[h as usize]) < 0) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[m] reaches the position i.
        let mut k = m;
        while k > i {
            data.swap(k as usize, (k - 1) as usize);
            k -= 1;
        }
        return;
    }

    let mid = (((a + b) as usize) >> 1) as isize;
    let n = mid + m;
    let (mut start, mut r);
    if m > mid {
        start = n - b;
        r = mid;
    } else {
        start = a;
        r = m;
    }
    let p = n - 1;

    while start < r {
        let c = (((start + r) as usize) >> 1) as isize;
        if !(cmp(&data[(p - c) as usize], &data[c as usize]) < 0) {
            start = c + 1;
        } else {
            r = c;
        }
    }

    let end = n - start;
    if start < m && m < end {
        rotate_cmp_func(data, start, m, end, cmp);
    }
    if a < start && start < mid {
        sym_merge_cmp_func(data, a, start, mid, cmp);
    }
    if mid < end && end < b {
        sym_merge_cmp_func(data, mid, end, b, cmp);
    }
}

// Go: slices/zsortanyfunc.go:464 rotateCmpFunc
/// rotateCmpFunc rotates two consecutive blocks u = data[a:m] and v = data[m:b] in data:
/// Data of the form 'x u v y' is changed to 'x v u y'.
/// rotate performs at most b-a many calls to data.Swap,
/// and it assumes non-degenerate arguments: a < m && m < b.
pub fn rotate_cmp_func<T, F: FnMut(&T, &T) -> i32>(
    data: &mut [T],
    a: isize,
    m: isize,
    b: isize,
    cmp: &mut F,
) {
    let mut i = m - a;
    let mut j = b - m;

    while i != j {
        if i > j {
            swap_range_cmp_func(data, m - i, m, j, cmp);
            i -= j;
        } else {
            swap_range_cmp_func(data, m - i, m + j - i, i, cmp);
            j -= i;
        }
    }
    // i == j
    swap_range_cmp_func(data, m - i, m, i, cmp);
}

// ---------------------------------------------------------------------------
// sort.Slice: sort/slice.go and sort/zsortfunc.go (generated from the same
// template as slices/zsortanyfunc.go, over a lessSwap instead of cmp)
// ---------------------------------------------------------------------------

// Go: sort/slice.go:24 Slice
/// Slice sorts the slice x given the provided less function.
///
/// The sort is not guaranteed to be stable: equal elements
/// may be reversed from their original order.
/// For a stable sort, use [SliceStable].
///
/// The less function must satisfy the same requirements as
/// the Interface type's Less method.
///
/// PORT: Go `less(i, j int)` reads `x[i]` and `x[j]`; here `less` gets
/// the two elements. Go's reflect swapper is `<[T]>::swap`. `sort_slice`
/// (`by_index`) is this sort with one copy for all element types; this
/// direct one is for the tests (no hot site sorts with `sort.Slice`).
#[cfg(test)]
fn sort_slice_direct<T, F: FnMut(&T, &T) -> bool>(x: &mut [T], less: F) {
    let length = x.len() as isize;
    let limit = bits_len(length as usize);
    let mut data = LessSwap {
        data: x,
        less_fn: less,
    };
    pdqsort_func(&mut data, 0, length, limit);
}

// Go: sort/sort.go:85 lessSwap
/// lessSwap is a pair of Less and Swap function for use with the
/// auto-generated func-optimized variant of sort.go in
/// zfuncversion.go.
pub struct LessSwap<'a, T, F: FnMut(&T, &T) -> bool> {
    pub data: &'a mut [T],
    pub less_fn: F,
}

impl<T, F: FnMut(&T, &T) -> bool> LessSwap<'_, T, F> {
    /// Go `data.Less(i, j)`.
    pub fn less(&mut self, i: isize, j: isize) -> bool {
        (self.less_fn)(&self.data[i as usize], &self.data[j as usize])
    }

    /// Go `data.Swap(i, j)`.
    pub fn swap(&mut self, i: isize, j: isize) {
        self.data.swap(i as usize, j as usize);
    }
}

// Go: sort/zsortfunc.go:10 insertionSort_func
/// insertionSort_func sorts data[a:b] using insertion sort.
pub fn insertion_sort_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) {
    let mut i = a + 1;
    while i < b {
        let mut j = i;
        while j > a && data.less(j, j - 1) {
            data.swap(j, j - 1);
            j -= 1;
        }
        i += 1;
    }
}

// Go: sort/zsortfunc.go:20 siftDown_func
/// siftDown_func implements the heap property on data[lo:hi].
/// first is an offset into the array where the root of the heap lies.
pub fn sift_down_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    lo: isize,
    hi: isize,
    first: isize,
) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            break;
        }
        if child + 1 < hi && data.less(first + child, first + child + 1) {
            child += 1;
        }
        if !data.less(first + root, first + child) {
            return;
        }
        data.swap(first + root, first + child);
        root = child;
    }
}

// Go: sort/zsortfunc.go:38 heapSort_func
pub fn heap_sort_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) {
    let first = a;
    let lo = 0;
    let hi = b - a;

    // Build heap with greatest element at top.
    let mut i = (hi - 1) / 2;
    while i >= 0 {
        sift_down_func(data, i, hi, first);
        i -= 1;
    }

    // Pop elements, largest first, into end of data.
    let mut i = hi - 1;
    while i >= 0 {
        data.swap(first, first + i);
        sift_down_func(data, lo, i, first);
        i -= 1;
    }
}

// Go: sort/zsortfunc.go:61 pdqsort_func
/// pdqsort_func sorts data[a:b].
/// The algorithm based on pattern-defeating quicksort(pdqsort), but without the optimizations from BlockQuicksort.
/// pdqsort paper: <https://arxiv.org/pdf/2106.05123.pdf>
/// C++ implementation: <https://github.com/orlp/pdqsort>
/// Rust implementation: <https://docs.rs/pdqsort/latest/pdqsort/>
/// limit is the number of allowed bad (very unbalanced) pivots before falling back to heapsort.
pub fn pdqsort_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    mut a: isize,
    mut b: isize,
    mut limit: isize,
) {
    const MAX_INSERTION: isize = 12;

    let mut was_balanced = true; // whether the last partitioning was reasonably balanced
    let mut was_partitioned = true; // whether the slice was already partitioned

    loop {
        let length = b - a;

        if length <= MAX_INSERTION {
            insertion_sort_func(data, a, b);
            return;
        }

        // Fall back to heapsort if too many bad choices were made.
        if limit == 0 {
            heap_sort_func(data, a, b);
            return;
        }

        // If the last partitioning was imbalanced, we need to breaking patterns.
        if !was_balanced {
            break_patterns_func(data, a, b);
            limit -= 1;
        }

        let (mut pivot, mut hint) = choose_pivot_func(data, a, b);
        if hint == SortedHint::DecreasingHint {
            reverse_range_func(data, a, b);
            // The chosen pivot was pivot-a elements after the start of the array.
            // After reversing it is pivot-a elements before the end of the array.
            // The idea came from Rust's implementation.
            pivot = (b - 1) - (pivot - a);
            hint = SortedHint::IncreasingHint;
        }

        // The slice is likely already sorted.
        if was_balanced && was_partitioned && hint == SortedHint::IncreasingHint {
            if partial_insertion_sort_func(data, a, b) {
                return;
            }
        }

        // Probably the slice contains many duplicate elements, partition the slice into
        // elements equal to and elements greater than the pivot.
        if a > 0 && !data.less(a - 1, pivot) {
            let mid = partition_equal_func(data, a, b, pivot);
            a = mid;
            continue;
        }

        let (mid, already_partitioned) = partition_func(data, a, b, pivot);
        was_partitioned = already_partitioned;

        let (left_len, right_len) = (mid - a, b - mid);
        let balance_threshold = length / 8;
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort_func(data, a, mid, limit);
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort_func(data, mid + 1, b, limit);
            b = mid;
        }
    }
}

// Go: sort/zsortfunc.go:135 partition_func
/// partition_func does one quicksort partition.
/// Let p = data[pivot]
/// Moves elements in data[a:b] around, so that data[i]<p and data[j]>=p for i<newpivot and j>newpivot.
/// On return, data[newpivot] = p
pub fn partition_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
    pivot: isize,
) -> (isize, bool) {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    while i <= j && data.less(i, a) {
        i += 1;
    }
    while i <= j && !data.less(j, a) {
        j -= 1;
    }
    if i > j {
        data.swap(j, a);
        return (j, true);
    }
    data.swap(i, j);
    i += 1;
    j -= 1;

    loop {
        while i <= j && data.less(i, a) {
            i += 1;
        }
        while i <= j && !data.less(j, a) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    data.swap(j, a);
    (j, false)
}

// Go: sort/zsortfunc.go:173 partitionEqual_func
/// partitionEqual_func partitions data[a:b] into elements equal to data[pivot] followed by elements greater than data[pivot].
/// It assumed that data[a:b] does not contain elements smaller than the data[pivot].
pub fn partition_equal_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
    pivot: isize,
) -> isize {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    loop {
        while i <= j && !data.less(a, i) {
            i += 1;
        }
        while i <= j && data.less(a, j) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    i
}

// Go: sort/zsortfunc.go:195 partialInsertionSort_func
/// partialInsertionSort_func partially sorts a slice, returns true if the slice is sorted at the end.
pub fn partial_insertion_sort_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) -> bool {
    const MAX_STEPS: isize = 5; // maximum number of adjacent out-of-order pairs that will get shifted
    const SHORTEST_SHIFTING: isize = 50; // don't shift any elements on short arrays
    let mut i = a + 1;
    let mut j = 0;
    while j < MAX_STEPS {
        while i < b && !data.less(i, i - 1) {
            i += 1;
        }

        if i == b {
            return true;
        }

        if b - a < SHORTEST_SHIFTING {
            return false;
        }

        data.swap(i, i - 1);

        // Shift the smaller one to the left.
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
                j -= 1;
            }
        }
        // Shift the greater one to the right.
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
                j += 1;
            }
        }
        j += 1;
    }
    false
}

// Go: sort/zsortfunc.go:240 breakPatterns_func
/// breakPatterns_func scatters some elements around in an attempt to break some patterns
/// that might cause imbalanced partitions in quicksort.
pub fn break_patterns_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) {
    let length = b - a;
    if length >= 8 {
        let mut random = Xorshift(length as u64);
        let modulus = next_power_of_two(length);

        let mut idx = a + (length / 4) * 2 - 1;
        while idx <= a + (length / 4) * 2 + 1 {
            let mut other = ((random.next() as usize) & (modulus - 1)) as isize;
            if other >= length {
                other -= length;
            }
            data.swap(idx, a + other);
            idx += 1;
        }
    }
}

// Go: sort/zsortfunc.go:261 choosePivot_func
/// choosePivot_func chooses a pivot in data[a:b].
///
/// [0,8): chooses a static pivot.
/// [8,shortestNinther): uses the simple median-of-three method.
/// [shortestNinther,∞): uses the Tukey ninther method.
pub fn choose_pivot_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) -> (isize, SortedHint) {
    const SHORTEST_NINTHER: isize = 50;
    const MAX_SWAPS: isize = 4 * 3;

    let l = b - a;

    let mut swaps: isize = 0;
    let mut i = a + l / 4 * 1;
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;

    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            // Tukey ninther method, the idea came from Rust's implementation.
            i = median_adjacent_func(data, i, &mut swaps);
            j = median_adjacent_func(data, j, &mut swaps);
            k = median_adjacent_func(data, k, &mut swaps);
        }
        // Find the median among i, j, k and stores it into j.
        j = median_func(data, i, j, k, &mut swaps);
    }

    match swaps {
        0 => (j, SortedHint::IncreasingHint),
        MAX_SWAPS => (j, SortedHint::DecreasingHint),
        _ => (j, SortedHint::UnknownHint),
    }
}

// Go: sort/zsortfunc.go:298 order2_func
/// order2_func returns x,y where data[x] <= data[y], where x,y=a,b or x,y=b,a.
pub fn order2_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
    swaps: &mut isize,
) -> (isize, isize) {
    if data.less(b, a) {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

// Go: sort/zsortfunc.go:307 median_func
/// median_func returns x where data[x] is the median of data[a],data[b],data[c], where x is a, b, or c.
pub fn median_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
    c: isize,
    swaps: &mut isize,
) -> isize {
    let (a, b) = order2_func(data, a, b, swaps);
    let (b, c) = order2_func(data, b, c, swaps);
    let _ = c;
    let (_a, b) = order2_func(data, a, b, swaps);
    b
}

// Go: sort/zsortfunc.go:315 medianAdjacent_func
/// medianAdjacent_func finds the median of data[a - 1], data[a], data[a + 1] and stores the index into a.
pub fn median_adjacent_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    swaps: &mut isize,
) -> isize {
    median_func(data, a - 1, a, a + 1, swaps)
}

// Go: sort/zsortfunc.go:319 reverseRange_func
pub fn reverse_range_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
) {
    let mut i = a;
    let mut j = b - 1;
    while i < j {
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
}

// Go: sort/zsortfunc.go:329 swapRange_func
pub fn swap_range_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    b: isize,
    n: isize,
) {
    let mut i = 0;
    while i < n {
        data.swap(a + i, b + i);
        i += 1;
    }
}

// Go: sort/zsortfunc.go:335 stable_func
pub fn stable_func<T, F: FnMut(&T, &T) -> bool>(data: &mut LessSwap<'_, T, F>, n: isize) {
    let mut block_size = 20; // must be > 0
    let (mut a, mut b) = (0, block_size);
    while b <= n {
        insertion_sort_func(data, a, b);
        a = b;
        b += block_size;
    }
    insertion_sort_func(data, a, n);

    while block_size < n {
        a = 0;
        b = 2 * block_size;
        while b <= n {
            sym_merge_func(data, a, a + block_size, b);
            a = b;
            b += 2 * block_size;
        }
        let m = a + block_size;
        if m < n {
            sym_merge_func(data, a, m, n);
        }
        block_size *= 2;
    }
}

// Go: sort/zsortfunc.go:378 symMerge_func
/// symMerge_func merges the two sorted subsequences data[a:m] and data[m:b]
/// (see `sym_merge_cmp_func`).
pub fn sym_merge_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    m: isize,
    b: isize,
) {
    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[a] into data[m:b]
    // if data[a:m] only contains one element.
    if m - a == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] >= data[a] for m <= i < b.
        // Exit the search loop with i == b in case no such index exists.
        let mut i = m;
        let mut j = b;
        while i < j {
            let h = (((i + j) as usize) >> 1) as isize;
            if data.less(h, a) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[a] reaches the position before i.
        let mut k = a;
        while k < i - 1 {
            data.swap(k, k + 1);
            k += 1;
        }
        return;
    }

    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[m] into data[a:m]
    // if data[m:b] only contains one element.
    if b - m == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] > data[m] for a <= i < m.
        // Exit the search loop with i == m in case no such index exists.
        let mut i = a;
        let mut j = m;
        while i < j {
            let h = (((i + j) as usize) >> 1) as isize;
            if !data.less(m, h) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[m] reaches the position i.
        let mut k = m;
        while k > i {
            data.swap(k, k - 1);
            k -= 1;
        }
        return;
    }

    let mid = (((a + b) as usize) >> 1) as isize;
    let n = mid + m;
    let (mut start, mut r);
    if m > mid {
        start = n - b;
        r = mid;
    } else {
        start = a;
        r = m;
    }
    let p = n - 1;

    while start < r {
        let c = (((start + r) as usize) >> 1) as isize;
        if !data.less(p - c, c) {
            start = c + 1;
        } else {
            r = c;
        }
    }

    let end = n - start;
    if start < m && m < end {
        rotate_func(data, start, m, end);
    }
    if a < start && start < mid {
        sym_merge_func(data, a, start, mid);
    }
    if mid < end && end < b {
        sym_merge_func(data, mid, end, b);
    }
}

// Go: sort/zsortfunc.go:464 rotate_func
/// rotate_func rotates two consecutive blocks u = data[a:m] and v = data[m:b] in data:
/// Data of the form 'x u v y' is changed to 'x v u y'.
/// rotate performs at most b-a many calls to data.Swap,
/// and it assumes non-degenerate arguments: a < m && m < b.
pub fn rotate_func<T, F: FnMut(&T, &T) -> bool>(
    data: &mut LessSwap<'_, T, F>,
    a: isize,
    m: isize,
    b: isize,
) {
    let mut i = m - a;
    let mut j = b - m;

    while i != j {
        if i > j {
            swap_range_func(data, m - i, m, j);
            i -= j;
        } else {
            swap_range_func(data, m - i, m + j - i, i);
            j -= i;
        }
    }
    // i == j
    swap_range_func(data, m - i, m, i);
}

/// The size classes of the Go allocator (internal/runtime/gc/sizeclasses.go
/// `SizeClassToSize`, go1.27.1, without class 0).
const GO_SIZE_CLASSES: [usize; 67] = [
    8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320, 352,
    384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792, 2048,
    2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472, 9728,
    10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264, 28672,
    32768,
];

// Go: runtime/slice.go nextslicecap (go1.27.1)
/// The capacity that growslice asks for before it rounds up to a size class.
fn next_slice_cap(new_len: usize, old_cap: usize) -> usize {
    let mut new_cap = old_cap;
    let double_cap = new_cap + new_cap;
    if new_len > double_cap {
        return new_len;
    }
    const THRESHOLD: usize = 256;
    if old_cap < THRESHOLD {
        return double_cap;
    }
    loop {
        // Transition from growing 2x for small slices to growing 1.25x for
        // large slices.
        new_cap += (new_cap + 3 * THRESHOLD) >> 2;
        if new_cap >= new_len {
            return new_cap;
        }
    }
}

// Go: runtime/msize.go roundupsize (go1.27.1, 64-bit)
/// The usable size of the block that mallocgc gives for `size` bytes. An
/// object with pointers (`noscan` false) over 512 bytes has an 8-byte malloc
/// header in its block.
fn round_up_size(size: usize, noscan: bool) -> usize {
    const MAX_SMALL_SIZE: usize = 32768;
    const MALLOC_HEADER_SIZE: usize = 8;
    const MIN_SIZE_FOR_MALLOC_HEADER: usize = 512;
    const PAGE_SIZE: usize = 8192;
    if size <= MAX_SMALL_SIZE - MALLOC_HEADER_SIZE {
        let header = if !noscan && size > MIN_SIZE_FOR_MALLOC_HEADER {
            MALLOC_HEADER_SIZE
        } else {
            0
        };
        let class = GO_SIZE_CLASSES
            .iter()
            .find(|&&class| class >= size + header);
        return class.expect("a small object fits a size class") - header;
    }
    // A large object takes whole pages.
    size.next_multiple_of(PAGE_SIZE)
}

// Go: runtime/slice.go growslice for a `[]byte` (noscan)
/// The capacity of a full Go `[]byte` with capacity `old_cap` after an
/// `append` that needs `new_len` bytes.
pub fn go_byte_slice_grow(old_cap: usize, new_len: usize) -> usize {
    round_up_size(next_slice_cap(new_len, old_cap), true)
}

// Go: runtime/slice.go growslice for a `[]*T` (elements of 8 bytes with
// pointers)
/// The capacity of a full Go `[]*T` with capacity `old_cap` after an
/// `append` that needs `new_len` elements: 1, 2, 4, 8, 16, 32, 64, 143, 287,
/// 607, 1023, ... when it grows by one each time.
pub fn go_ptr_slice_grow(old_cap: usize, new_len: usize) -> usize {
    round_up_size(next_slice_cap(new_len, old_cap) * 8, false) / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capacities of a `[]*T` that grows by one append at a time, as Go
    /// go1.27.1 prints them (`cap` after each growth).
    #[test]
    fn go_ptr_slice_grow_ladder() {
        let mut ladder = vec![];
        let mut cap = 0;
        while cap < 48128 {
            cap = go_ptr_slice_grow(cap, cap + 1);
            ladder.push(cap);
        }
        let go = [
            1, 2, 4, 8, 16, 32, 64, 143, 287, 607, 1023, 1535, 2303, 3071, 4095, 6144, 8192, 11264,
            14336, 18432, 23552, 29696, 37888, 48128,
        ];
        assert_eq!(ladder, go);
    }

    /// A `(file, pos, id)` key and the comparator shape of Go
    /// `compareSymbolsWorker`: files 0 and 1 are in the file index map
    /// (indexes 0 and 1), files 2 and 3 are not and read as index 0. So keys
    /// in files 0, 2 and 3 compare by position in one file and by id across
    /// files, which is not transitive.
    #[derive(Clone, Copy)]
    struct Key {
        file: i32,
        pos: i32,
        id: i32,
    }

    fn file_index(file: i32) -> i32 {
        i32::from(file == 1)
    }

    fn cmp_keys(a: &Key, b: &Key) -> i32 {
        if a.id == b.id {
            return 0;
        }
        if a.file != b.file {
            let r = file_index(a.file) - file_index(b.file);
            if r != 0 {
                return r;
            }
        } else if a.pos != b.pos {
            return a.pos - b.pos;
        }
        a.id - b.id
    }

    fn keys(n: i32) -> Vec<Key> {
        let mut seed: u32 = 12345;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as i32
        };
        (0..n)
            .map(|i| {
                let file = next() % 4;
                let pos = next() % 100;
                Key {
                    file,
                    pos,
                    id: n - i,
                }
            })
            .collect()
    }

    fn ids(keys: &[Key]) -> Vec<i32> {
        keys.iter().map(|k| k.id).collect()
    }

    // Rust std `sort_by` and `sort_unstable_by` (1.93.0) panic on these 64
    // keys: "user-provided comparison function does not correctly implement
    // a total order". The expected orders are the output of Go 1.26.8
    // `slices.SortFunc`, `slices.SortStableFunc` and `sort.Slice` on the same
    // keys and comparator.
    #[test]
    fn non_total_order_sorts_as_go() {
        let go_unstable = [
            13, 5, 1, 9, 3, 29, 14, 12, 16, 10, 17, 18, 24, 31, 4, 19, 2, 21, 28, 27, 32, 11, 37,
            26, 20, 41, 7, 35, 44, 39, 34, 46, 60, 47, 45, 51, 48, 57, 58, 36, 59, 62, 49, 54, 53,
            64, 25, 30, 22, 50, 40, 15, 8, 33, 43, 52, 55, 42, 56, 6, 61, 38, 23, 63,
        ];
        let go_stable = [
            13, 5, 1, 9, 3, 14, 12, 16, 10, 4, 2, 17, 18, 32, 11, 20, 7, 24, 35, 28, 27, 36, 37,
            26, 41, 44, 29, 59, 51, 48, 39, 31, 34, 19, 21, 60, 47, 45, 58, 62, 49, 46, 54, 57, 53,
            64, 25, 30, 22, 50, 40, 15, 8, 33, 43, 52, 55, 42, 56, 6, 61, 38, 23, 63,
        ];

        let mut a = keys(64);
        sort_func_hot(&mut a, cmp_keys);
        assert_eq!(ids(&a), go_unstable);
        let mut a = keys(64);
        sort_func(&mut a, cmp_keys);
        assert_eq!(ids(&a), go_unstable);

        let mut b = keys(64);
        sort_stable_func_hot(&mut b, cmp_keys);
        assert_eq!(ids(&b), go_stable);
        let mut b = keys(64);
        sort_stable_func(&mut b, cmp_keys);
        assert_eq!(ids(&b), go_stable);

        let mut c = keys(64);
        sort_slice_direct(&mut c, |x, y| cmp_keys(x, y) < 0);
        assert_eq!(ids(&c), go_unstable);
        let mut c = keys(64);
        sort_slice(&mut c, |x, y| cmp_keys(x, y) < 0);
        assert_eq!(ids(&c), go_unstable);
    }

    /// The index sorts (`by_index`) give the same permutation as the direct
    /// sorts, for a comparator that is not transitive and for one that
    /// answers at random: they make the same comparisons in the same order.
    #[test]
    fn index_sorts_match_direct_sorts() {
        for n in [0, 1, 2, 12, 13, 20, 21, 50, 64, 100, 300] {
            let input = keys(n);
            let (mut want, mut got) = (input.clone(), input.clone());
            sort_func_hot(&mut want, cmp_keys);
            by_index::sort_func(&mut got, cmp_keys);
            assert_eq!(ids(&got), ids(&want));
            let (mut want, mut got) = (input.clone(), input.clone());
            sort_stable_func_hot(&mut want, cmp_keys);
            by_index::sort_stable_func(&mut got, cmp_keys);
            assert_eq!(ids(&got), ids(&want));
            let (mut want, mut got) = (input.clone(), input.clone());
            sort_slice_direct(&mut want, |x, y| cmp_keys(x, y) < 0);
            by_index::sort_slice(&mut got, |x, y| cmp_keys(x, y) < 0);
            assert_eq!(ids(&got), ids(&want));

            let seed = n as u64 + 7;
            let (mut a, mut b) = (Xorshift(seed), Xorshift(seed));
            let (mut want, mut got) = (input.clone(), input.clone());
            sort_func_hot(&mut want, |_, _| (a.next() % 3) as i32 - 1);
            by_index::sort_func(&mut got, |_, _| (b.next() % 3) as i32 - 1);
            assert_eq!(ids(&got), ids(&want));
            let (mut want, mut got) = (input.clone(), input.clone());
            sort_stable_func_hot(&mut want, |_, _| (a.next() % 3) as i32 - 1);
            by_index::sort_stable_func(&mut got, |_, _| (b.next() % 3) as i32 - 1);
            assert_eq!(ids(&got), ids(&want));
            let (mut want, mut got) = (input.clone(), input);
            sort_slice_direct(&mut want, |_, _| a.next() % 2 == 0);
            by_index::sort_slice(&mut got, |_, _| b.next() % 2 == 0);
            assert_eq!(ids(&got), ids(&want));
        }
    }

    /// The index `stable_sort_by` gives the same order as std's `sort_by` for
    /// strict weak orders with many equal keys, at sizes that reach std's
    /// insertion sort and its driftsort.
    #[test]
    fn index_stable_sort_matches_sort_by() {
        for n in [0, 1, 2, 19, 20, 21, 50, 64, 100, 300, 1000, 5000] {
            let input = keys(n);
            let (mut want, mut got) = (input.clone(), input.clone());
            want.sort_by(|a, b| (a.file, a.pos).cmp(&(b.file, b.pos)));
            by_index::stable_sort_by(&mut got, |a, b| (a.file, a.pos).cmp(&(b.file, b.pos)));
            assert_eq!(ids(&got), ids(&want));
            let (mut want, mut got) = (input.clone(), input);
            want.sort_by(|a, b| b.file.cmp(&a.file));
            by_index::stable_sort_by(&mut got, |a, b| b.file.cmp(&a.file));
            assert_eq!(ids(&got), ids(&want));
        }
    }

    /// A comparator with a random answer on every call, at sizes that reach
    /// insertion sort, pdqsort, the heapsort fallback and symMerge. The sorts
    /// must not panic and must return a permutation.
    #[test]
    fn random_comparator_does_not_panic() {
        let mut rng = Xorshift(7);
        for n in [0, 1, 2, 12, 13, 20, 21, 50, 100, 300] {
            let input: Vec<u32> = (0..n).collect();
            let mut a = input.clone();
            sort_func_hot(&mut a, |_, _| (rng.next() % 3) as i32 - 1);
            let mut b = input.clone();
            sort_stable_func_hot(&mut b, |_, _| (rng.next() % 3) as i32 - 1);
            let mut c = input.clone();
            sort_slice_direct(&mut c, |_, _| rng.next() % 2 == 0);
            let mut d = input.clone();
            sort_func(&mut d, |_, _| (rng.next() % 3) as i32 - 1);
            let mut e = input.clone();
            sort_stable_func(&mut e, |_, _| (rng.next() % 3) as i32 - 1);
            let mut f = input.clone();
            sort_slice(&mut f, |_, _| rng.next() % 2 == 0);
            for mut out in [a, b, c, d, e, f] {
                out.sort_unstable();
                assert_eq!(out, input);
            }
        }
    }
}
