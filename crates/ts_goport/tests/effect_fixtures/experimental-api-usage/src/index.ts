import { beta, Box, dual, epsilon, gamma, over, Widget } from "./lib"
import { delta } from "./local"

// Allowed by the exact entry.
export const r1 = beta()
export const r2 = epsilon()
// Unstable, so only the unstable rule reports it.
export const r3 = gamma
// The selected overload decides.
export const r4 = over("s")
export const r5 = over(1)
export const r6 = dual("s")
export const r7 = dual(1)
export const r8 = new Widget()
export const r9 = Widget.create()
// A file with no package name gives the identifier name.
export const r10 = delta
export const open = (b: Box) => b.open()
export const close = (b: Box) => b.close()
