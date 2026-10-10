import { o1, fn1 } from "./a";
// Property access and call errors (symbolToString, signatureToString).
o1.nope;
fn1.nope;
fn1(1, { value: "x", map: (f) => f });
export function useIt<T extends { id: number }>(items: T[], pick: (t: T) => string) {
  const m = new Map<number, T>();
  for (const it of items) m.set(it.id, it);
  const bad: Map<string, T> = m;
  const bad2: (t: T) => number = pick;
  return [bad, bad2];
}
export class Wrapper<T> { constructor(public inner: T) {} unwrap(): T { return this.inner; } }
const w = new Wrapper({ deep: { list: [1, 2, 3], fn: (x: number) => x } });
const badW: Wrapper<{ deep: { list: string[] } }> = w;
// Declaration emit with types that need names from elsewhere (nested to-string in emit).
export const exported = () => w;
export const exported2 = { w, fn1, o1 };
