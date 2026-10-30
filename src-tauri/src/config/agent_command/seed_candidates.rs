use crate::config::config_seed::ConfigSeedCandidate;
use crate::config::placeholders::PlaceholderContext;

pub(crate) fn append_catalog_defaults(
    dest: &str,
    placeholder_context: Option<&PlaceholderContext>,
    os_token: Option<&str>,
    candidates: &mut Vec<ConfigSeedCandidate>,
) {
    // #769 P2 + #1318: append the absent-only, non-empty CatalogDefault
    // tier LAST (lowest precedence). Pure path math here;
    // perform_config_seed gates it on absent-dest + non-empty master,
    // so it never overwrites an existing replica config. The master
    // lives in the session workspace's `.ac/coding-agents/_seed/<dest>`
    // (replicas always have a workspace); a session without a workspace
    // falls back to the legacy `<config_dir>/coding-agents/_seed/<dest>`
    // location. The master exists only for built-ins that ship one; for
    // any other dest the candidate path is absent and the tier is inert.
    let dest = dest.trim();
    let master_root = placeholder_context
        .and_then(|ctx| ctx.ac_root.as_ref())
        .cloned()
        .or_else(crate::config::config_dir);
    if let Some(master_root) = master_root {
        // #2162: the OS variant of the master precedes the base one,
        // inside the same (absent-only, non-empty-gated) tier.
        if let Some(os) = os_token {
            candidates.push(crate::config::config_seed::ConfigSeedCandidate {
                tier: crate::config::config_seed::ConfigSeedTier::CatalogDefault,
                os_specific: true,
                path: crate::config::coding_agents_catalog::master_dir_for_dest(
                    &master_root,
                    &format!("{dest}.{os}"),
                ),
            });
        }
        candidates.push(crate::config::config_seed::ConfigSeedCandidate {
            tier: crate::config::config_seed::ConfigSeedTier::CatalogDefault,
            os_specific: false,
            path: crate::config::coding_agents_catalog::master_dir_for_dest(&master_root, dest),
        });
    }
}
