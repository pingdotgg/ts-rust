import path from 'node:path';
import { Counter, greet, Mood } from './greeter';
import type { Person } from './greeter';
import { calls, logged } from './logged';

class Shop {
  constructor(private readonly people: Person[]) {}

  @logged
  greetAll(prefix: string): string[] {
    return this.people.map((person) => `${prefix} ${greet(person)}`);
  }
}

async function main(): Promise<void> {
  const shop = new Shop([{ name: 'Ada' }, { name: 'Linus', mood: Mood.Grumpy }]);
  const counter = new Counter<string>().add('a').add('b').add('a');
  const lazy = await import('./greeter');
  const result = {
    greetings: shop.greetAll('>'),
    counts: counter.entries(),
    calls,
    lazyMood: lazy.Mood.Grumpy,
    file: path.basename(__filename),
  };
  console.log(JSON.stringify(result));
}

void main();
