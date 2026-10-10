import { fn1 } from "./a";
export declare function useIt<T extends {
    id: number;
}>(items: T[], pick: (t: T) => string): (Map<string, T> | ((t: T) => number))[];
export declare class Wrapper<T> {
    inner: T;
    constructor(inner: T);
    unwrap(): T;
}
export declare const exported: () => Wrapper<{
    deep: {
        list: number[];
        fn: (x: number) => number;
    };
}>;
export declare const exported2: {
    w: Wrapper<{
        deep: {
            list: number[];
            fn: (x: number) => number;
        };
    }>;
    fn1: typeof fn1;
    o1: {
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
};
