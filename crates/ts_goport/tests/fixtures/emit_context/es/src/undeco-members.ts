declare function dec(v: any, c: any): any;
@dec
export class E {
  /** m doc */ m(): void {}
  /** p doc */ p: number = 1;
  "lit"(): void {}
}
export class F {
  @dec /** a doc */ a(): void {}
  /** b doc */ b(): void {}
  @dec
  /** c doc */
  "c-lit"(): void {}
  @dec
  /** n doc */
  42(): void {}
}
