declare function p(t: any, k: any, i: number): void;
export class B {
  constructor(@p { a, b }: { a: number; b: number }, @p [c]: number[]) {}
}
