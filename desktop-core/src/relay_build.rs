//! Build selection for commands carrying only the `--dev` flag. Profile
//! commands carry an explicit socket and do not borrow either pointer.
use agent_relay_node::relay::outbound::Build;

pub(crate) fn hook_build(development: bool) -> Build {
    if development {
        Build::Development
    } else {
        Build::Production
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_selects_exactly_its_own_build() {
        assert!(matches!(hook_build(false), Build::Production));
        assert!(matches!(hook_build(true), Build::Development));
    }
}
