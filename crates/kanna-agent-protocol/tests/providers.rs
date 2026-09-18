use kanna_agent_protocol::{agent_provider_specs, AgentProvider, AgentSessionType, EffortOverride};
use std::str::FromStr;

#[test]
fn registry_covers_every_provider_once() {
    let specs = agent_provider_specs();
    assert_eq!(specs.len(), AgentProvider::ALL.len());
    for provider in AgentProvider::ALL {
        assert_eq!(specs.iter().filter(|spec| spec.id == provider).count(), 1);
    }
}

#[test]
fn provider_metadata_matches_runtime_contracts() {
    assert_eq!(AgentProvider::Antigravity.executable(), "agy");
    for provider in AgentProvider::ALL {
        assert_eq!(provider.default_session_type(), AgentSessionType::Pty);
    }
    assert!(AgentProvider::Claude.supports_headless());
    assert!(AgentProvider::Codex.supports_headless());
    assert!(AgentProvider::Opencode.supports_headless());
    assert!(!AgentProvider::Copilot.supports_headless());
    assert!(!AgentProvider::Antigravity.supports_headless());
}

#[test]
fn provider_model_override_flags_are_explicit() {
    assert_eq!(AgentProvider::Claude.model_override_flag(), Some("--model"));
    assert_eq!(
        AgentProvider::Copilot.model_override_flag(),
        Some("--model")
    );
    assert_eq!(AgentProvider::Codex.model_override_flag(), Some("-m"));
    assert_eq!(AgentProvider::Opencode.model_override_flag(), Some("-m"));
    assert_eq!(AgentProvider::Antigravity.model_override_flag(), None);
}

#[test]
fn provider_effort_controls_and_native_values_are_explicit() {
    assert_eq!(
        AgentProvider::Codex.effort_override(),
        EffortOverride::Config("model_reasoning_effort")
    );
    assert_eq!(
        AgentProvider::Claude.effort_override(),
        EffortOverride::Flag("--effort")
    );
    assert_eq!(
        AgentProvider::Copilot.effort_override(),
        EffortOverride::Flag("--effort")
    );
    assert_eq!(
        AgentProvider::Opencode.effort_override(),
        EffortOverride::Flag("--variant")
    );
    assert_eq!(
        AgentProvider::Antigravity.effort_override(),
        EffortOverride::Flag("--effort")
    );
    assert_eq!(AgentProvider::Codex.effort_values(), None);
    assert_eq!(
        AgentProvider::Claude.effort_values(),
        Some(&["low", "medium", "high", "xhigh", "max"][..])
    );
    assert_eq!(
        AgentProvider::Copilot.effort_values(),
        Some(&["none", "minimal", "low", "medium", "high", "xhigh", "max"][..])
    );
    assert_eq!(AgentProvider::Opencode.effort_values(), None);
    assert_eq!(
        AgentProvider::Antigravity.effort_values(),
        Some(&["low", "medium", "high"][..])
    );
}

#[test]
fn provider_strings_round_trip() {
    for provider in AgentProvider::ALL {
        assert_eq!(
            AgentProvider::from_str(provider.as_str()).unwrap(),
            provider
        );
        assert_eq!(
            serde_json::from_str::<AgentProvider>(&serde_json::to_string(&provider).unwrap())
                .unwrap(),
            provider
        );
    }
    assert!(AgentProvider::from_str("future-agent").is_err());
}

#[test]
fn shared_structured_selection_contract() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../src/selection_cases.json")).unwrap();
    for case in cases {
        let result = serde_json::from_value::<Vec<kanna_agent_protocol::AgentSelectionEntry>>(
            case["value"].clone(),
        )
        .map_err(|e| e.to_string())
        .and_then(|entries| {
            kanna_agent_protocol::validate_agent_selection(&entries, true)?;
            Ok(entries
                .iter()
                .map(|entry| {
                    let e = entry.resolve(true).unwrap();
                    let mut v = serde_json::json!({"provider":e.provider});
                    if let Some(model) = e.model {
                        v["model"] = model.into();
                    }
                    if let Some(effort) = e.effort {
                        v["effort"] = effort.into();
                    }
                    if let Some(autocompact) = e.autocompact {
                        v["autocompact"] = autocompact.into();
                    }
                    v
                })
                .collect::<Vec<_>>())
        });
        if case["error"] == true {
            assert!(result.is_err(), "{case}");
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap()).unwrap(),
                case["expected"],
                "{case}"
            );
        }
    }
}

/// The auto-compact window is Claude-only, and the values Kanna accepts are
/// the ones measured against the CLI in
/// `tests/cli-contract/fixtures/claude-autocompact.json`. The fixture is the
/// same file the offline and live CLI-contract tests read, so a CLI that
/// widens the window or changes the flag surfaces in all three places.
mod autocompact {
    use kanna_agent_protocol::{
        resolve_autocompact_window, validate_provider_autocompact, AgentCandidate, AgentProvider,
        AgentSelectionEntry, DEFAULT_AUTOCOMPACT_WINDOW, MAX_AUTOCOMPACT_TOKENS,
        MIN_AUTOCOMPACT_TOKENS,
    };

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AcceptedWindow {
        value: String,
        tokens: Option<u64>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AutocompactContract {
        flag: String,
        default: String,
        min_tokens: u64,
        max_tokens: u64,
        accepted: Vec<AcceptedWindow>,
        rejected: Vec<String>,
    }

    fn contract() -> AutocompactContract {
        serde_json::from_str(include_str!(
            "../../../tests/cli-contract/fixtures/claude-autocompact.json"
        ))
        .unwrap()
    }

    #[test]
    fn only_claude_publishes_an_autocompact_flag() {
        assert_eq!(
            AgentProvider::Claude.autocompact_flag(),
            Some(contract().flag.as_str())
        );
        for provider in AgentProvider::ALL {
            if provider == AgentProvider::Claude {
                continue;
            }
            assert_eq!(
                provider.autocompact_flag(),
                None,
                "{provider} must not receive an autocompact flag"
            );
        }
    }

    #[test]
    fn the_measured_cli_bounds_are_what_kanna_validates_against() {
        let contract = contract();
        assert_eq!(DEFAULT_AUTOCOMPACT_WINDOW, contract.default);
        assert_eq!(MIN_AUTOCOMPACT_TOKENS, contract.min_tokens);
        assert_eq!(MAX_AUTOCOMPACT_TOKENS, contract.max_tokens);
    }

    #[test]
    fn every_measured_value_resolves_to_the_window_the_cli_reported() {
        for accepted in contract().accepted {
            assert_eq!(
                resolve_autocompact_window(&accepted.value),
                Ok(accepted.tokens),
                "accepted: {}",
                accepted.value
            );
        }
    }

    #[test]
    fn every_measured_usage_error_is_refused_before_the_spawn() {
        for rejected in contract().rejected {
            assert!(
                resolve_autocompact_window(&rejected).is_err(),
                "rejected: {rejected}"
            );
        }
    }

    #[test]
    fn a_bare_number_is_thousands_only_up_to_one_thousand() {
        // `200` and `200000` are the same window; `5000` is five thousand
        // tokens and out of range. Pinning it here keeps the two spellings
        // from quietly swapping meaning.
        assert_eq!(resolve_autocompact_window("200"), Ok(Some(200_000)));
        assert_eq!(resolve_autocompact_window("200000"), Ok(Some(200_000)));
        assert!(resolve_autocompact_window("5000").is_err());
    }

    #[test]
    fn a_window_beside_another_harness_is_refused_rather_than_dropped() {
        assert!(validate_provider_autocompact(AgentProvider::Claude, Some("400k")).is_ok());
        for provider in AgentProvider::ALL {
            if provider == AgentProvider::Claude {
                continue;
            }
            let error = validate_provider_autocompact(provider, Some("400k"))
                .expect_err("a non-claude harness must refuse an autocompact window");
            assert!(error.contains("not supported"), "{error}");
            assert!(validate_provider_autocompact(provider, None).is_ok());
        }
    }

    #[test]
    fn a_structured_candidate_carries_its_window_and_validates_its_harness() {
        let claude = AgentSelectionEntry::Candidate(AgentCandidate {
            harness: AgentProvider::Claude,
            model: Some("opus".to_string()),
            effort: None,
            autocompact: Some("400k".to_string()),
        })
        .resolve(false)
        .unwrap();
        assert_eq!(claude.autocompact.as_deref(), Some("400k"));

        assert!(AgentSelectionEntry::Candidate(AgentCandidate {
            harness: AgentProvider::Codex,
            model: None,
            effort: None,
            autocompact: Some("400k".to_string()),
        })
        .resolve(false)
        .is_err());

        // Out of range is refused at the same point, so a bad repo config
        // fails the request rather than the CLI's argument parser.
        assert!(AgentSelectionEntry::Candidate(AgentCandidate {
            harness: AgentProvider::Claude,
            model: None,
            effort: None,
            autocompact: Some("5000".to_string()),
        })
        .resolve(false)
        .is_err());
    }

    #[test]
    fn a_compact_selector_never_names_a_window() {
        let selector = kanna_agent_protocol::parse_provider_selector("claude-fable-hi").unwrap();
        assert_eq!(selector.autocompact, None);
    }
}
