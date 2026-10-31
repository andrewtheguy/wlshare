use super::*;

#[test]
fn a_scroll_has_the_documented_layout() {
    assert_eq!(scroll(-3, 120), [0xE5, 0, 0xFF, 0xFD, 0x00, 0x78]);
    assert_eq!(parse(&[0xE5, 0, 0xFF, 0xFD, 0x00, 0x78, 0xFF]), Some(((-3, 120), 6)));
    assert_eq!(parse(&scroll(1, 1)[..5]), None);
}
