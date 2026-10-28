export enum Mood {
  Happy = 'happy',
  Grumpy = 'grumpy',
}

export interface Person {
  name: string;
  mood?: Mood;
}

export function greet(person: Person): string {
  const mood = person.mood ?? Mood.Happy;
  return `Hello, ${person.name} (${mood})`;
}

export class Counter<T> {
  private readonly counts = new Map<T, number>();

  add(item: T): this {
    this.counts.set(item, (this.counts.get(item) ?? 0) + 1);
    return this;
  }

  entries(): [T, number][] {
    return [...this.counts.entries()].sort(([, a], [, b]) => b - a);
  }
}
