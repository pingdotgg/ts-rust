var __decorate = (this && this.__decorate) || function (decorators, target, key, desc) {
    var c = arguments.length, r = c < 3 ? target : desc === null ? desc = Object.getOwnPropertyDescriptor(target, key) : desc, d;
    if (typeof Reflect === "object" && typeof Reflect.decorate === "function") r = Reflect.decorate(decorators, target, key, desc);
    else for (var i = decorators.length - 1; i >= 0; i--) if (d = decorators[i]) r = (c < 3 ? d(r) : c > 3 ? d(target, key, r) : d(target, key)) || r;
    return c > 3 && r && Object.defineProperty(target, key, r), r;
};
var __param = (this && this.__param) || function (paramIndex, decorator) {
    return function (target, key) { decorator(target, key, paramIndex); }
};
function dec(...args) { }
let C = class C {
    y;
    m() { }
    p = 1;
    static s() { }
    get g() { return 1; }
    constructor(x, y) {
        this.y = y;
    }
};
__decorate([
    dec /** m doc */
], C.prototype, "m", null);
__decorate([
    dec /** p doc */
], C.prototype, "p", void 0);
__decorate([
    dec /** g doc */
], C.prototype, "g", null);
__decorate([
    dec /** s doc */
], C, "s", null);
C = __decorate([
    __param(0, dec),
    __param(1, dec)
], C);
export { C };
//# sourceMappingURL=names.js.map