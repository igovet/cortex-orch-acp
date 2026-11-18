//! The route for one short owner-facing text (a task title, a commit message, a
//! pull request text): the cheapest model of the connection whose plan limit is
//! the least used. Only data the runtime already has decides it:
//!
//! - "Cheapest" is the owner's routing tier placement (`models.<engine>.tiers`):
//!   `light` before `standard`, `deep`, `flagship`. A model with no placement has
//!   no known cost and is never chosen. Ties keep the model id order.
//! - "Least used" is the same plan-window figure route ranking reads
//!   (`RouteResolver::usage`): the highest used percentage of a connection's live
//!   windows. A connection without a reading is unknown, never zero.
//! - Connections with a reading below 95 % come first, lowest first; then those
//!   without a reading; then those at or above 95 %. Equal keys keep the fixed
//!   engine order (`claude-agent-acp`, then `codex-acp`) and then connection id.
//!
//! Only routes the owner allows for automatic choice (enabled, not denied, in
//! `auto`, with a tier) on the Claude and Codex ACP engines are offered. The
//! result is an ordinary [`SelectedRoute`], so its launch is the same hardened
//! [`SelectedRoute::session_launch`] every engine session uses. Nothing here
//! starts a process.
use super::{Candidate, RouteResolver, SelectedRoute};
use serde_json::json;

const TIER_ORDER: [&str; 4] = ["light", "standard", "deep", "flagship"];
/// ACP engines a short text may run on.
pub const ONE_SHOT_ENGINES: [&str; 2] = ["claude-agent-acp", "codex-acp"];
const HOT: f64 = 95.0;

/// What was chosen and why, for diagnostics. Holds no credential or path.
#[derive(Clone, Debug, PartialEq)]
pub struct ShortTextRoute {
    pub engine: String,
    pub model: String,
    pub connection: String,
    pub tier: &'static str,
    pub used: Option<f64>,
    /// Why this connection: `only_connection`, `least_used`,
    /// `usage_unknown_fixed_order` or `all_near_limit`.
    pub reason: &'static str,
}

/// One allowed (connection, model) pair with what is known about it.
#[derive(Clone, Debug)]
pub struct Offer {
    pub engine: String,
    pub model: String,
    pub connection: String,
    /// Index into [`TIER_ORDER`]: 0 is the cheapest.
    pub tier: usize,
    pub used: Option<f64>,
}

fn bucket(used: Option<f64>) -> u8 {
    match used {
        Some(n) if n < HOT => 0,
        None => 1,
        Some(_) => 2,
    }
}
fn engine_rank(engine: &str) -> usize {
    ONE_SHOT_ENGINES
        .iter()
        .position(|e| *e == engine)
        .unwrap_or(ONE_SHOT_ENGINES.len())
}

/// The deterministic choice over `offers`: each connection's cheapest model,
/// then the least used connection. Returns the index and the reason.
pub fn choose(offers: &[Offer]) -> Option<(usize, &'static str)> {
    // Cheapest model per connection.
    let mut best: Vec<usize> = Vec::new();
    for (i, o) in offers.iter().enumerate() {
        match best
            .iter()
            .position(|b| offers[*b].connection == o.connection)
        {
            None => best.push(i),
            Some(at) => {
                let b = &offers[best[at]];
                if (o.tier, &o.model) < (b.tier, &b.model) {
                    best[at] = i;
                }
            }
        }
    }
    best.sort_by(|a, b| {
        let (a, b) = (&offers[*a], &offers[*b]);
        bucket(a.used)
            .cmp(&bucket(b.used))
            .then(
                a.used
                    .unwrap_or(0.0)
                    .partial_cmp(&b.used.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(engine_rank(&a.engine).cmp(&engine_rank(&b.engine)))
            .then(a.connection.cmp(&b.connection))
    });
    let first = *best.first()?;
    let reason = if best.len() == 1 {
        "only_connection"
    } else {
        match bucket(offers[first].used) {
            0 => "least_used",
            1 => "usage_unknown_fixed_order",
            _ => "all_near_limit",
        }
    };
    Some((first, reason))
}

impl RouteResolver {
    fn short_text_offer(&self, c: &Candidate) -> Option<Offer> {
        if !ONE_SHOT_ENGINES.contains(&c.engine())
            || !self.allowed(c, false)
            || self.automatic(c).is_some()
            || c.connection["credential_ref"]["kind"] != "engine_native_dir"
        {
            return None;
        }
        let placed = self.placed(c);
        let tier = TIER_ORDER.iter().position(|t| placed.contains(t))?;
        Some(Offer {
            engine: c.engine().to_owned(),
            model: c.model().to_owned(),
            connection: c.connection_id().to_owned(),
            tier,
            used: self.usage(c).ok().flatten(),
        })
    }
    /// The route for one short text, or `None` when no allowed route can run one.
    /// The selected route carries `thought_level=low` when the catalogue offers it.
    pub fn short_text_route(&self) -> Option<(ShortTextRoute, SelectedRoute)> {
        let mut offers = Vec::new();
        let mut owners = Vec::new();
        for c in &self.candidates {
            if let Some(offer) = self.short_text_offer(c) {
                offers.push(offer);
                owners.push(c);
            }
        }
        let (i, reason) = choose(&offers)?;
        let (o, c) = (&offers[i], owners[i]);
        let low_effort = c.entry["route"]["supported_parameters"]
            .as_array()
            .is_some_and(|pairs| {
                pairs
                    .iter()
                    .any(|p| p[0] == "thought_level" && p[1] == "low")
            });
        let parameters = if low_effort {
            json!({"thought_level":"low"})
        } else {
            json!({})
        };
        let selected = self.selected(c, parameters, "short_text").ok()?;
        let route = ShortTextRoute {
            engine: o.engine.clone(),
            model: o.model.clone(),
            connection: o.connection.clone(),
            tier: TIER_ORDER[o.tier],
            used: o.used,
            reason,
        };
        Some((route, selected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offer(engine: &str, model: &str, connection: &str, tier: usize, used: Option<f64>) -> Offer {
        Offer {
            engine: engine.into(),
            model: model.into(),
            connection: connection.into(),
            tier,
            used,
        }
    }
    const CLAUDE: &str = "claude-agent-acp";
    const CODEX: &str = "codex-acp";

    #[test]
    fn the_least_used_connection_gives_its_cheapest_model() {
        let offers = [
            offer(CLAUDE, "opus", "cn_a", 3, Some(40.0)),
            offer(CLAUDE, "haiku", "cn_a", 0, Some(40.0)),
            offer(CODEX, "gpt-luna", "cn_b", 0, Some(12.0)),
            offer(CODEX, "gpt-sol", "cn_b", 1, Some(12.0)),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].model.as_str(), reason),
            ("gpt-luna", "least_used")
        );
        let offers = [
            offer(CLAUDE, "haiku", "cn_a", 0, Some(5.0)),
            offer(CODEX, "gpt-luna", "cn_b", 0, Some(60.0)),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!((offers[i].model.as_str(), reason), ("haiku", "least_used"));
    }
    #[test]
    fn without_a_light_model_the_next_cheapest_tier_is_used() {
        let offers = [
            offer(CLAUDE, "opus", "cn_a", 3, None),
            offer(CLAUDE, "sonnet", "cn_a", 1, None),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].model.as_str(), reason),
            ("sonnet", "only_connection")
        );
    }
    #[test]
    fn a_missing_reading_is_unknown_not_zero_and_follows_the_fixed_order() {
        // A known reading wins over an unknown one, however high below the limit.
        let offers = [
            offer(CLAUDE, "haiku", "cn_a", 0, None),
            offer(CODEX, "gpt-luna", "cn_b", 0, Some(80.0)),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].connection.as_str(), reason),
            ("cn_b", "least_used")
        );
        // No readings at all: engine order, then connection id.
        let offers = [
            offer(CODEX, "gpt-luna", "cn_a", 0, None),
            offer(CLAUDE, "haiku", "cn_z", 0, None),
            offer(CLAUDE, "haiku", "cn_m", 0, None),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].connection.as_str(), reason),
            ("cn_m", "usage_unknown_fixed_order")
        );
        // A connection at its limit goes after an unknown one.
        let offers = [
            offer(CLAUDE, "haiku", "cn_a", 0, Some(97.0)),
            offer(CODEX, "gpt-luna", "cn_b", 0, None),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].connection.as_str(), reason),
            ("cn_b", "usage_unknown_fixed_order")
        );
        let offers = [
            offer(CLAUDE, "haiku", "cn_a", 0, Some(99.0)),
            offer(CODEX, "gpt-luna", "cn_b", 0, Some(96.0)),
        ];
        let (i, reason) = choose(&offers).unwrap();
        assert_eq!(
            (offers[i].connection.as_str(), reason),
            ("cn_b", "all_near_limit")
        );
    }
    #[test]
    fn nothing_offered_chooses_nothing() {
        assert_eq!(choose(&[]), None);
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::super::{Candidate, RouteEnvironment, RouteResolver};
    use serde_json::json;

    fn candidate(engine: &str, connection: &str, model: &str, low: bool) -> Candidate {
        let pairs = if low {
            json!([["thought_level", "low"]])
        } else {
            json!([])
        };
        Candidate {
            connection: json!({"id":connection,"credential_ref":{"kind":"engine_native_dir","path":format!("/nonexistent/{connection}")}}),
            entry: json!({"route":{"engine":engine,"model":model,"id":format!("{engine}/{model}"),"supported_parameters":pairs}}),
        }
    }
    /// Only owner-allowed automatic routes with a tier on a one-shot engine are
    /// offered; without plan readings the fixed engine order decides.
    #[test]
    fn the_owner_policy_decides_what_may_write_a_short_text() {
        let rules = json!({
            "enabled":{"claude-agent-acp":["haiku","opus","manual"],"codex-acp":["luna","sol"],"opencode":["cheap"]},
            "deny":["claude-agent-acp/haiku"],
            "models":{
                "claude-agent-acp":{"tiers":{"haiku":["light"],"opus":["flagship"],"manual":["light"]},"auto":["haiku","opus"]},
                "codex-acp":{"tiers":{"luna":["light"],"sol":["standard"]},"auto":["luna","sol"]},
                "opencode":{"tiers":{"cheap":["light"]},"auto":["cheap"]}
            }
        });
        let resolver = RouteResolver {
            environment: RouteEnvironment {
                home: "/nonexistent".into(),
                config: "/nonexistent".into(),
                state: "/nonexistent".into(),
                routing_override: None,
            },
            rules,
            defaults: json!({}),
            candidates: vec![
                candidate("opencode", "cn_000000000000", "cheap", false),
                candidate("codex-acp", "cn_00000000000b", "sol", false),
                candidate("codex-acp", "cn_00000000000b", "luna", true),
                candidate("claude-agent-acp", "cn_00000000000a", "haiku", false),
                candidate("claude-agent-acp", "cn_00000000000a", "manual", false),
                candidate("claude-agent-acp", "cn_00000000000a", "opus", false),
            ],
        };
        // haiku is denied and manual is not automatic: Claude offers only opus,
        // Codex offers luna. Neither has a reading, so Claude comes first.
        let (route, selected) = resolver.short_text_route().unwrap();
        assert_eq!(
            (
                route.engine.as_str(),
                route.model.as_str(),
                route.tier,
                route.reason
            ),
            (
                "claude-agent-acp",
                "opus",
                "flagship",
                "usage_unknown_fixed_order"
            )
        );
        assert_eq!(route.used, None);
        assert_eq!(
            (selected.engine(), selected.model(), selected.connection()),
            ("claude-agent-acp", "opus", "cn_00000000000a")
        );
        assert!(selected.parameters.is_empty());
        // Without Claude, Codex gives its cheapest model, with the low effort it offers.
        let mut resolver = resolver;
        resolver
            .candidates
            .retain(|c| c.engine() != "claude-agent-acp");
        let (route, selected) = resolver.short_text_route().unwrap();
        assert_eq!(
            (route.model.as_str(), route.reason),
            ("luna", "only_connection")
        );
        assert_eq!(
            selected.parameters.get("thought_level").map(String::as_str),
            Some("low")
        );
        resolver.candidates.retain(|c| c.engine() == "opencode");
        assert!(resolver.short_text_route().is_none());
    }
}
