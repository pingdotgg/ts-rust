declare const env: { [key: string]: string | undefined };
delete env.FOO;
delete env["BAR"];
delete (env.PAREN);
delete (((env.PARENS)));
delete env?.OPTIONAL;
delete (env?.OPTIONAL_PAREN);

declare const numbers: { [key: string]: number };
delete numbers.x;

declare const nested: { inner: { [key: string]: string | undefined } };
delete nested.inner.x;

declare const optional: { a?: string };
delete optional.a;

declare const el: HTMLElement;
delete el.dataset.x;

export const value = delete env.USED;
