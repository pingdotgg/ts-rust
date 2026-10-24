export const calls: string[] = [];

export function logged(target: object, key: string, descriptor: PropertyDescriptor): void {
  const original = descriptor.value as (...args: unknown[]) => unknown;
  descriptor.value = function (this: unknown, ...args: unknown[]) {
    calls.push(`${target.constructor.name}.${key}(${args.length})`);
    return original.apply(this, args);
  };
}
