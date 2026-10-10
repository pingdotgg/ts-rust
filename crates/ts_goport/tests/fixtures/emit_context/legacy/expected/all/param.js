var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __param = (this && this.__param) || function (paramIndex, decorator) {
    return function (target, key) { decorator(target, key, paramIndex); }
};
let A = class A {
    constructor(x, /* c1 */ y, ...rest) { }
    m(a, b) { }
    static s(q = 1) { }
};
__decorate([
    __param(0, p)
], A.prototype, "m", null);
__decorate([
    __param(0, p)
], A, "s", null);
A = __decorate([
    __param(0, p),
    __param(1, p),
    __param(2, p)
], A);
export { A };
export function plain(x, y) { return x; }
//# sourceMappingURL=param.js.map