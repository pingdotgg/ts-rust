import { Service, IService, Mode, Alias } from "./types";
import type { Alias as TA } from "./types";
function dec(...args: any[]): any { return () => {}; }
function pdec(t: any, k: any, i: number) {}
/** decorated class */
@dec()
export class Ctl {
  @dec() svc: Service;
  @dec() iface: IService;
  @dec() mode: Mode;
  @dec() alias: Alias;
  @dec() ta: TA;
  @dec() promise: Promise<number>;
  @dec() arr: Service[];
  @dec() fnT: () => void;
  constructor(@pdec s: Service, @pdec private readonly m: Mode, @pdec a?: Alias) {}
  @dec() method(@pdec x: Service, y: IService): Promise<Service> { return null; }
  @dec() get acc(): Mode { return Mode.A; }
  set acc(v: Mode) {}
  @dec() static st(x: symbol, y: bigint): void {}
}
export const inst = new Ctl(new Service(), Mode.B);
