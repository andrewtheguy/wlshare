use super::*;

#[test]
fn fixed_point_carries_fractions_exactly() {
    assert_eq!(to_fixed(2.0), 0x0002_0000);
    assert_eq!(to_fixed(1.5), 0x0001_8000);
    assert_eq!(from_fixed(0x0001_8000), 1.5);
}

#[test]
fn output_scale_has_the_documented_layout() {
    assert_eq!(output_scale(3456, 1802, 2.0), [0xE0, 0, 0x0D, 0x80, 0x07, 0x0A, 0x00, 0x02, 0x00, 0x00]);
    assert_eq!(client_density(3456, 1802, 2.0), output_scale(3456, 1802, 2.0), "the two directions share it");
}

#[test]
fn a_message_parses_once_it_is_all_there() {
    let msg = client_density(1280, 800, 1.5);
    assert_eq!(Density::parse(&msg[..9]), None);
    assert_eq!(Density::parse(&[&msg[..], &[0xFF]].concat()), Some((Density { width: 1280, height: 800, fixed: 0x0001_8000 }, 10)));
}
