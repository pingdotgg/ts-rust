// Module resolve hook of register.mjs.

const shim = new URL("./channel-shim.mjs", import.meta.url).href;
const syncChannel = /\/api\/syncChannel\.(ts|js)$/;

export async function resolve(specifier, context, nextResolve) {
    const resolved = await nextResolve(specifier, context);
    if (syncChannel.test(resolved.url)) {
        return { url: shim, format: "module", shortCircuit: true };
    }
    return resolved;
}
