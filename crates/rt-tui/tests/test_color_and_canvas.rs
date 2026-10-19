use ratatui::style::{Color, Modifier};
use rt_tui::display::{color_map::parse_color_definition, Canvas, ColorKind, ColorMap};

#[test]
fn test_color_definition_parsing() {
    let s1 = parse_color_definition("bold white on blue").unwrap();
    assert_eq!(s1.fg, Some(Color::Gray));
    assert_eq!(s1.bg, Some(Color::Blue));
    assert!(s1.add_modifier.contains(Modifier::BOLD));

    let s2 = parse_color_definition("reverse bright yellow on 16").unwrap();
    assert_eq!(s2.fg, Some(Color::LightYellow));
    assert_eq!(s2.bg, Some(Color::Indexed(16)));
    assert!(s2.add_modifier.contains(Modifier::REVERSED));

    let s3 = parse_color_definition("underline dim red").unwrap();
    assert_eq!(s3.fg, Some(Color::Red));
    assert!(s3.add_modifier.contains(Modifier::UNDERLINED));
    assert!(s3.add_modifier.contains(Modifier::DIM));
}

#[test]
fn test_colormap_parity_mixing() {
    let mut map = ColorMap::new();
    map.build_from_definitions([
        ("ui.color.title", "bold white on blue"),
        ("ui.color.even", "on 234"),
        ("ui.color.odd", "on 236"),
    ]);

    let base = map.get_style(ColorKind::Title, None);
    assert_eq!(base.bg, Some(Color::Blue));

    // Even row has base background blue (not overridden because base bg is already set)
    let even = map.get_style(ColorKind::Title, Some(false));
    assert_eq!(even.bg, Some(Color::Blue));

    // An element with no background gets the odd/even background:
    let s_info = parse_color_definition("green").unwrap();
    map.set_style(ColorKind::Info, s_info);

    let even_info = map.get_style(ColorKind::Info, Some(false));
    assert_eq!(even_info.bg, Some(Color::Indexed(234)));

    let odd_info = map.get_style(ColorKind::Info, Some(true));
    assert_eq!(odd_info.bg, Some(Color::Indexed(236)));
}

#[test]
fn test_canvas_drawing_and_rows() {
    let mut canvas = Canvas::new(0, 0, 20, 3);
    canvas.print_at(0, 0, "Hello World");
    canvas.print_at(2, 1, "Line 2");
    canvas.print_at(0, 2, "Last line 1234567890123"); // should be truncated at 20 chars

    let rows = canvas.to_string_rows();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].starts_with("Hello World"));
    assert!(rows[1].starts_with("  Line 2"));
    assert_eq!(rows[2].len(), 20);
    assert_eq!(&rows[2], "Last line 1234567890");
}
