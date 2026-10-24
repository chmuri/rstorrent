use rt_rpc::parse_command_spec;

#[test]
fn parses_a_bare_getter_with_empty_arguments() {
    let (name, args, rest) = parse_command_spec("d.hash=").unwrap();
    assert_eq!(name, "d.hash");
    assert!(rest.is_empty());
    assert!(
        args.is_empty(),
        "`name=` must produce an empty argument object so the multicall loop can skip per-target evaluation"
    );
}

#[test]
fn keeps_arguments_and_the_remainder_for_the_caller() {
    let (name, args, rest) = parse_command_spec("d.name= value ;next=1").unwrap();
    assert_eq!(name, "d.name");
    assert_eq!(args.as_str().unwrap(), "value");
    assert_eq!(rest, "next=1");
}

#[test]
fn blank_and_comment_input_produce_an_empty_name() {
    for input in ["", "   ", "   # comment"] {
        let (name, _args, _rest) = parse_command_spec(input).unwrap();
        assert!(name.is_empty(), "input {input:?}");
    }
}

#[test]
fn a_missing_equals_is_an_error() {
    assert!(parse_command_spec("d.hash").is_err());
}

#[test]
fn unicode_arguments_are_parsed_without_slicing_panics() {
    let (name, args, _rest) = parse_command_spec("d.name= żółć ").unwrap();
    assert_eq!(name, "d.name");
    assert_eq!(args.as_str().unwrap(), "żółć");
}
