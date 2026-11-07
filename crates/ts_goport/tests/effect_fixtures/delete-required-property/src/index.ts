declare const required: { a: string; b: string; c: string; d?: string };
delete required.a;
delete (required.b);
delete required?.c;
delete required.d;

declare const readonlyOptional: { readonly a?: string };
delete readonlyOptional.a;

export {};
