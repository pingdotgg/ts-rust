//! Go `internal/project/extendedconfigcache.go`.

use crate::project::prelude::*;

use xxhash_rust::xxh3::Xxh3;

// Go: project/extendedconfigcache.go:10 ExtendedConfigParseArgs
// PORT: the cache stores its parse function for any args, so the args own
// their values: Go interfaces are `Rc<dyn ..>` (a nil `Cache` is `None`).
// Go passes the config file registry builder as `Host` and `Cache`, so the
// builder hands out `Rc`s of itself.
pub struct ExtendedConfigParseArgs {
    pub file_name: String,
    pub content: String,
    pub fs: Rc<dyn FileSource>,
    pub resolution_stack: Vec<tspath::Path>,
    pub host: Rc<dyn tsoptions::ParseConfigHost>,
    pub cache: Option<Rc<dyn tsoptions::ExtendedConfigCache>>,
}

// Go: project/extendedconfigcache.go:19 ExtendedConfigCacheEntry
// PORT: Go embeds `*tsoptions.ExtendedConfigCacheEntry`; here it is the
// field `extended_config_cache_entry` (map-project.md section 4). Go
// `xxh3.Uint128` is `u128`.
#[derive(Clone, Debug)]
pub struct ExtendedConfigCacheEntry {
    pub extended_config_cache_entry: Rc<tsoptions::ExtendedConfigCacheEntry>,
    pub hash: u128,
}

// Go: project/extendedconfigcache.go:24 ExtendedConfigCache
pub type ExtendedConfigCache =
    OwnerCache<tspath::Path, Rc<ExtendedConfigCacheEntry>, ExtendedConfigParseArgs>;

// Go: project/extendedconfigcache.go:26 NewExtendedConfigCache
pub fn new_extended_config_cache() -> Rc<ExtendedConfigCache> {
    new_owner_cache(
        |path: &tspath::Path, args: ExtendedConfigParseArgs| -> Rc<ExtendedConfigCacheEntry> {
            let mut result = ExtendedConfigCacheEntry {
                extended_config_cache_entry: Rc::new(tsoptions::parse_extended_config(
                    &args.file_name,
                    path.clone(),
                    &args.resolution_stack,
                    &*args.host,
                    args.cache.as_deref(),
                )),
                hash: 0,
            };
            result.hash = hash(&result.extended_config_cache_entry, &args);
            Rc::new(result)
        },
        Some(Box::new(
            |_path: &tspath::Path,
             entry: &Rc<ExtendedConfigCacheEntry>,
             args: &ExtendedConfigParseArgs|
             -> bool {
                entry.hash == 0 || entry.hash != hash(&entry.extended_config_cache_entry, args)
            },
        )),
    )
}

// Go: project/extendedconfigcache.go:41 hash
// PORT: Go passes the args by value; here by reference.
pub fn hash(entry: &tsoptions::ExtendedConfigCacheEntry, args: &ExtendedConfigParseArgs) -> u128 {
    let mut hasher = Xxh3::new();
    hasher.update(args.content.as_bytes());
    for file_name in entry.extended_file_names() {
        let Some(fh) = args.fs.get_file(file_name) else {
            return 0;
        };
        hasher.update(fh.content().as_bytes());
    }
    hasher.digest128()
}
