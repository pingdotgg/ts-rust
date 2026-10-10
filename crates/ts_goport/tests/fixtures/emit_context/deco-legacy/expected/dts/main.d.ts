import { Service, IService, Mode, Alias } from "./types";
import type { Alias as TA } from "./types";
/** decorated class */
export declare class Ctl {
    private readonly m;
    svc: Service;
    iface: IService;
    mode: Mode;
    alias: Alias;
    ta: TA;
    promise: Promise<number>;
    arr: Service[];
    fnT: () => void;
    constructor(s: Service, m: Mode, a?: Alias);
    method(x: Service, y: IService): Promise<Service>;
    get acc(): Mode;
    set acc(v: Mode);
    static st(x: symbol, y: bigint): void;
}
export declare const inst: Ctl;
//# sourceMappingURL=main.d.ts.map