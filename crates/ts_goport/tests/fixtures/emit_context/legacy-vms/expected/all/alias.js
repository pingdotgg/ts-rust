var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var C_1;
let C = class C {
    static { C_1 = this; }
    p;
    static s = 1;
    m(x = C_1.s, k) { return x + k; }
    static n(y = C_1) { return y; }
    constructor(p = C_1.s) {
        this.p = p;
    }
};
C = C_1 = __decorate([
    dec
], C);
export { C };
//# sourceMappingURL=alias.js.map