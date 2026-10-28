//! Node-API link setup (napi-build): the library loads into node, which
//! provides the Node-API symbols.

fn main() {
    napi_build::setup();
}
