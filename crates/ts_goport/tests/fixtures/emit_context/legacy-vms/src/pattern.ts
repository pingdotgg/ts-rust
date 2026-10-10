declare function p(t: any, k: any, i: number): void;
export class B {
  constructor(@p { a = 1, b }: { a?: number; b: number }, @p [c = 2]: number[]) {}
  m(@p { d = 3 }: { d?: number }) {}
}
