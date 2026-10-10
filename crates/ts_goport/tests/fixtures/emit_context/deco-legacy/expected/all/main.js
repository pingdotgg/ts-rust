"use strict";
var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __metadata = (this && this.__metadata) || function (k, v) {
    if (typeof Reflect === "object" && typeof Reflect.metadata === "function") return Reflect.metadata(k, v);
};
var __param = (this && this.__param) || function (paramIndex, decorator) {
    return function (target, key) { decorator(target, key, paramIndex); }
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.inst = exports.Ctl = void 0;
const types_1 = require("./types");
function dec(...args) { return () => { }; }
function pdec(t, k, i) { }
/** decorated class */
let Ctl = class Ctl {
    constructor(s, m, a) {
        this.m = m;
    }
    method(x, y) { return null; }
    get acc() { return types_1.Mode.A; }
    set acc(v) { }
    static st(x, y) { }
};
exports.Ctl = Ctl;
__decorate([
    dec(),
    __metadata("design:type", types_1.Service)
], Ctl.prototype, "svc", void 0);
__decorate([
    dec(),
    __metadata("design:type", Object)
], Ctl.prototype, "iface", void 0);
__decorate([
    dec(),
    __metadata("design:type", Number)
], Ctl.prototype, "mode", void 0);
__decorate([
    dec(),
    __metadata("design:type", Object)
], Ctl.prototype, "alias", void 0);
__decorate([
    dec(),
    __metadata("design:type", Object)
], Ctl.prototype, "ta", void 0);
__decorate([
    dec(),
    __metadata("design:type", Promise)
], Ctl.prototype, "promise", void 0);
__decorate([
    dec(),
    __metadata("design:type", Array)
], Ctl.prototype, "arr", void 0);
__decorate([
    dec(),
    __metadata("design:type", Function)
], Ctl.prototype, "fnT", void 0);
__decorate([
    dec(),
    __param(0, pdec),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [types_1.Service, Object]),
    __metadata("design:returntype", Promise)
], Ctl.prototype, "method", null);
__decorate([
    dec(),
    __metadata("design:type", Number),
    __metadata("design:paramtypes", [Number])
], Ctl.prototype, "acc", null);
__decorate([
    dec(),
    __metadata("design:type", Function),
    __metadata("design:paramtypes", [Symbol, BigInt]),
    __metadata("design:returntype", void 0)
], Ctl, "st", null);
exports.Ctl = Ctl = __decorate([
    dec(),
    __param(0, pdec),
    __param(1, pdec),
    __param(2, pdec),
    __metadata("design:paramtypes", [types_1.Service, Number, Object])
], Ctl);
exports.inst = new Ctl(new types_1.Service(), types_1.Mode.B);
//# sourceMappingURL=main.js.map