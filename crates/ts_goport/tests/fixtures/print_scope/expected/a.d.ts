declare const s: unique symbol;
type Big = {
    a: string;
    b: number;
    c: boolean[];
    d: {
        e: [string, number?];
        f: readonly string[];
    };
    g?: () => void;
    h: Record<string, number>;
    [k: `x${string}`]: number;
};
type Mapped<T> = {
    readonly [K in keyof T]?: T[K] extends Function ? never : T[K];
};
type LateKeys = {
    [K in typeof s]: number;
};
interface Box<T> {
    value: T;
    map<U>(f: (x: T) => U): Box<U>;
}
declare enum Color {
    Red = 0,
    Green = 1,
    Blue = 2
}
declare class Klass {
    private p;
    protected q: string;
    r: Color;
    method<T extends object>(x: T): keyof T;
}
export declare const o1: {
    a: number;
    b: string;
    c: boolean[];
    d: {
        e: [string, number];
        f: string[];
    };
    h: {
        k: number;
    };
};
export declare const bad1: Big;
export declare const bad2: Mapped<Big>;
export declare const bad3: Big;
export declare const bad4: {
    a: number;
    z: string;
};
export declare const fn1: <T>(x: T, y: Box<T>) => Box<readonly [T, T]>;
export declare const bad5: (x: number) => string;
export declare const bad6: <U>(x: U) => U;
export declare const bad7: Box<string>;
export declare const late: LateKeys;
export declare const bad8: {
    other: number;
};
export declare const bad9: string;
export declare const bad10: {
    s: number;
};
export declare const bad11: {
    p: number;
    q: string;
};
export declare const bad12: Klass;
export declare const bad13: (x: unknown) => x is number;
export declare const r1: number;
export declare const r2: string;
type LongUnion = "a1" | "a2" | "a3" | "a4" | "a5" | "a6" | "a7" | "a8" | "a9" | "a10" | "a11" | "a12" | "a13" | "a14" | "a15" | "a16" | "a17" | "a18" | "a19" | "a20" | "a21" | "a22" | "a23" | "a24" | "a25" | "a26" | "a27" | "a28" | "a29" | "a30" | "a31" | "a32" | "a33" | "a34" | "a35" | "a36" | "a37" | "a38" | "a39" | "a40" | "a41" | "a42" | "a43" | "a44" | "a45" | "a46" | "a47" | "a48" | "a49" | "a50";
export declare const bad14: {
    a: {
        b: {
            c: {
                d: {
                    e: {
                        f: {
                            g: {
                                h: {
                                    i: {
                                        j: {
                                            k: {
                                                l: 5;
                                            };
                                        };
                                    };
                                };
                            };
                        };
                    };
                };
            };
        };
    };
};
export declare const bad15: number;
export declare const bad16: Record<LongUnion, number>;
export declare const bad17: {
    x: Big;
    y: (x: number) => string;
};
export declare const bad18: {
    x: Big;
    y: (x: number) => string;
};
export {};
