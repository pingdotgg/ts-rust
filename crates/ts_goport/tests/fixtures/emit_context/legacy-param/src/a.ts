declare function p(t: any, k: any, i: number): void;
export class A {
  constructor(@p x: number, /* c1 */ @p y?: string, @p ...rest: number[]) {}
  m(@p a: number, b: string) {}
  static s(@p q = 1) {}
}
export function plain(x: number, y: string) { return x; }
