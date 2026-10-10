function dec(...args: any[]): any {}
export class C {
  @dec /** m doc */ m(): void {}
  @dec /** p doc */ p: number = 1;
  @dec /** s doc */ static s(): void {}
  @dec /** g doc */ get g(): number { return 1; }
  constructor(@dec /** x doc */ x: number, @dec public y: string) {}
}
