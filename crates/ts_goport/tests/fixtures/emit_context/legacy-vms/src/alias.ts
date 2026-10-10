declare function dec(t: any): any;
@dec
export class C {
  static s = 1;
  m(x: number = C.s, k: number) { return x + k; }
  static n(y: typeof C = C) { return y; }
  constructor(public p: number = C.s) {}
}
