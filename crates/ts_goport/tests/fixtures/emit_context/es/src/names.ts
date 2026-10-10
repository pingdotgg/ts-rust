function dec(v: any, c: any): any { return v; }
export class C {
  @dec /** m doc */ m(): void {}
  @dec /** p doc */ p: number = 1;
  @dec
  /** q doc */
  q(): void {}
  @dec /** s doc */ static s(): void {}
  @dec /** acc doc */ accessor a = 1;
  @dec /** g doc */ get g(): number { return 1; }
}
