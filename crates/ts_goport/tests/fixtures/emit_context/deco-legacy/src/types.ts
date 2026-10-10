export interface IService { run(): void }
export class Service implements IService { run() {} }
export enum Mode { A, B }
export type Alias = string | number;
