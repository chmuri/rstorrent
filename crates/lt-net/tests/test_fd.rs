use lt_net::fd::{fd_valid_flags, fd_valid_flags_raw, FdFlags};

#[test]
fn test_valid_flags() {
    assert!(fd_valid_flags(FdFlags::STREAM));
    assert!(fd_valid_flags(FdFlags::DATAGRAM));
    assert!(!fd_valid_flags(FdFlags::STREAM | FdFlags::DATAGRAM));

    assert!(fd_valid_flags(FdFlags::STREAM | FdFlags::NONBLOCK));
    assert!(fd_valid_flags(FdFlags::STREAM | FdFlags::REUSE_ADDRESS));
    assert!(fd_valid_flags(FdFlags::STREAM | FdFlags::V4_ONLY));
    assert!(fd_valid_flags(FdFlags::STREAM | FdFlags::V6_ONLY));

    assert!(fd_valid_flags(FdFlags::DATAGRAM | FdFlags::NONBLOCK));
    assert!(fd_valid_flags(FdFlags::DATAGRAM | FdFlags::REUSE_ADDRESS));
    assert!(fd_valid_flags(FdFlags::DATAGRAM | FdFlags::V4_ONLY));
    assert!(fd_valid_flags(FdFlags::DATAGRAM | FdFlags::V6_ONLY));

    assert!(!fd_valid_flags(FdFlags::V4_ONLY | FdFlags::V6_ONLY));
    assert!(!fd_valid_flags(FdFlags::STREAM | FdFlags::V4_ONLY | FdFlags::V6_ONLY));
    assert!(!fd_valid_flags(FdFlags::DATAGRAM | FdFlags::V4_ONLY | FdFlags::V6_ONLY));

    assert!(!fd_valid_flags_raw(0));
    assert!(!fd_valid_flags_raw(!FdFlags::ALL.bits()));
    assert!(!fd_valid_flags_raw(FdFlags::STREAM.bits() | !FdFlags::ALL.bits()));
    assert!(!fd_valid_flags_raw(0x3245132));
}
