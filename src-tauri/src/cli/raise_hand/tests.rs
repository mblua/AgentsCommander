use super::*;

#[test]
fn true_response_interprets_true() {
    assert_eq!(
        interpret_raise_hand_response(r#"{"raised":true}"#),
        Ok(true)
    );
}

#[test]
fn false_response_interprets_false() {
    assert_eq!(
        interpret_raise_hand_response(r#"{"raised":false}"#),
        Ok(false)
    );
}

#[test]
fn missing_or_non_bool_raised_maps_to_exit_two() {
    assert_eq!(
        interpret_raise_hand_response(r#"{"status":"raised"}"#),
        Err(2)
    );
    assert_eq!(
        interpret_raise_hand_response(r#"{"raised":"true"}"#),
        Err(2)
    );
    assert_eq!(interpret_raise_hand_response("not json"), Err(2));
}

#[test]
fn raise_hand_args_parse_with_default_timeout() {
    use clap::Parser;
    let parsed = crate::cli::Cli::try_parse_from([
        "agentscommander",
        "raise-hand",
        "--token",
        "11111111-1111-1111-1111-111111111111",
        "--root",
        "C:\\x",
    ])
    .expect("clap should accept raise-hand with token and root");
    let cmd = parsed.command.expect("subcommand present");
    match cmd {
        crate::cli::Commands::RaiseHand(args) => {
            assert_eq!(args.timeout, 15);
            assert_eq!(
                args.token.as_deref(),
                Some("11111111-1111-1111-1111-111111111111")
            );
            assert_eq!(args.root.as_deref(), Some("C:\\x"));
        }
        _ => panic!("expected RaiseHand subcommand"),
    }
}
