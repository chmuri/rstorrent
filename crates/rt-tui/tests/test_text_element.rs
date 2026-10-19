use lt_common::Object;
use ratatui::style::Style;
use rt_rpc::{CommandMap, Target};
use rt_tui::display::{
    format_ddhhmm, format_ddmmyyyy, format_hhmmss, format_kb, format_mb, format_percentage,
    format_timer, format_xb, TextElement, TextElementCommand, TextElementList, TextElementString,
    TextElementValue, FLAG_ESCAPE_HEX, FLAG_FIXED_WIDTH, FLAG_TIMER, FLAG_XB,
};

#[test]
fn test_time_and_date_formatting() {
    assert_eq!(format_hhmmss(0), " 0:00:00");
    assert_eq!(format_hhmmss(3661), " 1:01:01");
    assert_eq!(format_hhmmss(7325), " 2:02:05");

    assert_eq!(format_ddhhmm(3600), " 0d  1:00");
    assert_eq!(format_ddhhmm(90060), " 1d  1:01");
    assert_eq!(format_ddhhmm(100 * 24 * 3600), "--d --:--");

    // 2021-01-01 00:00:00 UTC = 1609459200
    assert_eq!(format_ddmmyyyy(1609459200), "01/01/2021");
    // 2024-02-29 (leap year) = 1709164800
    assert_eq!(format_ddmmyyyy(1709164800), "29/02/2024");
}

#[test]
fn test_size_formatting() {
    assert_eq!(format_kb(1024), "  1.0");
    assert_eq!(format_mb(10485760), "    10.0");

    assert_eq!(format_xb(512 * 1024), "512.0 KB");
    assert_eq!(format_xb(100 * 1024 * 1024), "100.0 MB");
    let two_gb = 2i64 * 1024 * 1024 * 1024;
    assert_eq!(format_xb(two_gb), "  2.0 GB");

    assert_eq!(format_timer(0), "--:--:--");
    assert_eq!(format_timer(3665), " 1:01:05");

    assert_eq!(format_percentage(50, 100), "[50%]");
    assert_eq!(format_percentage(100, 100), "[100%]");
}

#[test]
fn test_text_element_string_and_hex() {
    let elem = TextElementString::new("hello");
    let target = Target::None;
    assert_eq!(elem.render(&target, None), "hello");

    let mut elem_hex = TextElementString::new("ABC");
    elem_hex.flags = FLAG_ESCAPE_HEX;
    assert_eq!(elem_hex.render(&target, None), "414243");
}

#[test]
fn test_text_element_value() {
    let target = Target::None;
    let elem_xb = TextElementValue::from_const(50 * 1024 * 1024, FLAG_XB, Style::default());
    assert_eq!(elem_xb.render(&target, None), " 50.0 MB");

    let elem_timer = TextElementValue::from_const(3600, FLAG_TIMER, Style::default());
    assert_eq!(elem_timer.render(&target, None), " 1:00:00");
}

#[test]
fn test_text_element_command() {
    let mut map = CommandMap::new();
    map.insert_func(
        "test.greet",
        |_target, _args| Ok(Object::from("Welcome")),
        0,
        "",
        "",
    )
    .unwrap();

    let target = Target::None;
    let cmd = TextElementCommand::new("test.greet", 0, Style::default());
    assert_eq!(cmd.render(&target, Some(&map)), "Welcome");

    let cmd_fixed = TextElementCommand::with_length(
        "test.greet",
        FLAG_FIXED_WIDTH,
        Style::default(),
        10,
    );
    assert_eq!(cmd_fixed.render(&target, Some(&map)), "Welcome   ");
}

#[test]
fn test_text_element_list_columns() {
    let target = Target::None;
    let mut list = TextElementList::new();
    list.push(Box::new(TextElementString::new("Col1")));
    list.push(Box::new(TextElementString::new("Col2")));
    list.set_column(2);
    list.set_column_width(Some(8));

    let rendered = list.render(&target, None);
    assert_eq!(rendered, "Col1    Col2    ");
    assert_eq!(rendered.len(), 16);
}
