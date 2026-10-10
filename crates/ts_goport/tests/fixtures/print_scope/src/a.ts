// Assignability errors that print big types (relater error display).
declare const s: unique symbol;
type Big = { a: string; b: number; c: boolean[]; d: { e: [string, number?]; f: readonly string[] }; g?: () => void; h: Record<string, number>; [k: `x${string}`]: number };
type Mapped<T> = { readonly [K in keyof T]?: T[K] extends Function ? never : T[K] };
type Cond<T> = T extends string ? { s: T } : T extends number ? { n: T } : never;
type LateKeys = { [K in typeof s]: number };
interface Box<T> { value: T; map<U>(f: (x: T) => U): Box<U>; }
enum Color { Red, Green, Blue }
class Klass { private p = 1; protected q = "x"; r: Color = Color.Red; method<T extends object>(x: T): keyof T { return null!; } }

export const o1 = { a: 1, b: "x", c: [true], d: { e: ["q", 1] as [string, number], f: ["z"] }, h: { k: 1 } };
export const bad1: Big = o1;
export const bad2: Mapped<Big> = o1;
export const bad3: Big = o1;
export const bad4: { a: number; z: string } = o1;

export const fn1 = <T,>(x: T, y: Box<T>) => y.map((v) => [v, x] as const);
export const bad5: (x: number) => string = fn1;
export const bad6: <U>(x: U) => U = fn1;
export const bad7: Box<string> = fn1;

export const late: LateKeys = { [s]: 1 };
export const bad8: { other: number } = late;
export const bad9: string = late;

declare const c1: Cond<"a" | 1 | true>;
export const bad10: { s: number } = c1;

declare const k: Klass;
export const bad11: { p: number; q: string } = k;
export const bad12: Klass = { r: Color.Green, method: () => "x" };

declare function isStr(x: unknown): x is string;
export const bad13: (x: unknown) => x is number = isStr;

declare function over(x: string): string;
declare function over(x: number, y?: Box<number>): number;
declare function over<T extends Big>(x: T, y: Mapped<T>): T;
export const r1 = over({ a: 1 } as unknown as Big, { z: 1 });
export const r2 = over(true);

type LongUnion = "a1" | "a2" | "a3" | "a4" | "a5" | "a6" | "a7" | "a8" | "a9" | "a10" | "a11" | "a12" | "a13" | "a14" | "a15" | "a16" | "a17" | "a18" | "a19" | "a20" | "a21" | "a22" | "a23" | "a24" | "a25" | "a26" | "a27" | "a28" | "a29" | "a30" | "a31" | "a32" | "a33" | "a34" | "a35" | "a36" | "a37" | "a38" | "a39" | "a40" | "a41" | "a42" | "a43" | "a44" | "a45" | "a46" | "a47" | "a48" | "a49" | "a50";
type D0 = { l: LongUnion }; type D1 = { k: D0 }; type D2 = { j: D1 }; type D3 = { i: D2 }; type D4 = { h: D3 }; type D5 = { g: D4 }; type D6 = { f: D5 }; type D7 = { e: D6 }; type D8 = { d: D7 }; type D9 = { c: D8 }; type Deep = { a: { b: D9 } };
declare const deep: Deep;
export const bad14: { a: { b: { c: { d: { e: { f: { g: { h: { i: { j: { k: { l: 5 } } } } } } } } } } } } = deep;
export const bad15: number = deep;
export const bad16: Record<LongUnion, number> = { a1: 1 };

const o2 = { x: o1, y: fn1, z: late, w: k };
export const bad17: { x: Big; y: (x: number) => string } = o2;
export const bad18: { x: Big; y: (x: number) => string } = o2;
