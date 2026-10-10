var __runInitializers = (this && this.__runInitializers) || function (thisArg, initializers, value) {
    var useValue = arguments.length > 2;
    for (var i = 0; i < initializers.length; i++) {
        value = useValue ? initializers[i].call(thisArg, value) : initializers[i].call(thisArg);
    }
    return useValue ? value : void 0;
};
var __esDecorate = (this && this.__esDecorate) || function (ctor, descriptorIn, decorators, contextIn, initializers, extraInitializers) {
    function accept(f) { if (f !== void 0 && typeof f !== "function") throw new TypeError("Function expected"); return f; }
    var kind = contextIn.kind, key = kind === "getter" ? "get" : kind === "setter" ? "set" : "value";
    var target = !descriptorIn && ctor ? contextIn["static"] ? ctor : ctor.prototype : null;
    var descriptor = descriptorIn || (target ? Object.getOwnPropertyDescriptor(target, contextIn.name) : {});
    var _, done = false;
    for (var i = decorators.length - 1; i >= 0; i--) {
        var context = {};
        for (var p in contextIn) context[p] = p === "access" ? {} : contextIn[p];
        for (var p in contextIn.access) context.access[p] = contextIn.access[p];
        context.addInitializer = function (f) { if (done) throw new TypeError("Cannot add initializers after decoration has completed"); extraInitializers.push(accept(f || null)); };
        var result = (0, decorators[i])(kind === "accessor" ? { get: descriptor.get, set: descriptor.set } : descriptor[key], context);
        if (kind === "accessor") {
            if (result === void 0) continue;
            if (result === null || typeof result !== "object") throw new TypeError("Object expected");
            if (_ = accept(result.get)) descriptor.get = _;
            if (_ = accept(result.set)) descriptor.set = _;
            if (_ = accept(result.init)) initializers.unshift(_);
        }
        else if (_ = accept(result)) {
            if (kind === "field") initializers.unshift(_);
            else descriptor[key] = _;
        }
    }
    if (target) Object.defineProperty(target, contextIn.name, descriptor);
    done = true;
};
function dec(v, c) { return v; }
let C = (() => {
    let _staticExtraInitializers = [];
    let _instanceExtraInitializers = [];
    let _static_s_decorators;
    let _m_decorators;
    let _p_decorators;
    let _p_initializers = [];
    let _p_extraInitializers = [];
    let _q_decorators;
    let _a_decorators;
    let _a_initializers = [];
    let _a_extraInitializers = [];
    let _get_g_decorators;
    return class C {
        static {
            const _metadata = typeof Symbol === "function" && Symbol.metadata ? Object.create(null) : void 0;
            _m_decorators = [dec];
            _p_decorators = [dec];
            _q_decorators = [dec];
            _static_s_decorators = [dec];
            _a_decorators = [dec];
            _get_g_decorators = [dec];
            __esDecorate(this, null, _static_s_decorators, { kind: "method", name: "s", static: true, private: false, access: { has: obj => "s" in obj, get: obj => obj.s }, metadata: _metadata }, null, _staticExtraInitializers);
            __esDecorate(this, null, _m_decorators, { kind: "method", name: "m", static: false, private: false, access: { has: obj => "m" in obj, get: obj => obj.m }, metadata: _metadata }, null, _instanceExtraInitializers);
            __esDecorate(this, null, _q_decorators, { kind: "method", name: "q", static: false, private: false, access: { has: obj => "q" in obj, get: obj => obj.q }, metadata: _metadata }, null, _instanceExtraInitializers);
            __esDecorate(this, null, _a_decorators, { kind: "accessor", name: "a", static: false, private: false, access: { has: obj => "a" in obj, get: obj => obj.a, set: (obj, value) => { obj.a = value; } }, metadata: _metadata }, _a_initializers, _a_extraInitializers);
            __esDecorate(this, null, _get_g_decorators, { kind: "getter", name: "g", static: false, private: false, access: { has: obj => "g" in obj, get: obj => obj.g }, metadata: _metadata }, null, _instanceExtraInitializers);
            __esDecorate(null, null, _p_decorators, { kind: "field", name: "p", static: false, private: false, access: { has: obj => "p" in obj, get: obj => obj.p, set: (obj, value) => { obj.p = value; } }, metadata: _metadata }, _p_initializers, _p_extraInitializers);
            if (_metadata) Object.defineProperty(this, Symbol.metadata, { enumerable: true, configurable: true, writable: true, value: _metadata });
            __runInitializers(this, _staticExtraInitializers);
        }
        m() { }
        p = (__runInitializers(this, _instanceExtraInitializers), __runInitializers(this, _p_initializers, 1));
        q() { }
        static s() { }
        #a_accessor_storage = (__runInitializers(this, _p_extraInitializers), __runInitializers(this, _a_initializers, 1));
        get a() { return this.#a_accessor_storage; }
        set a(value) { this.#a_accessor_storage = value; }
        get g() { return 1; }
        constructor() {
            __runInitializers(this, _a_extraInitializers);
        }
    };
})();
export { C };
//# sourceMappingURL=names.js.map