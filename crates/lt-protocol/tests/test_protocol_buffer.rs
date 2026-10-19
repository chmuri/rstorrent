use lt_protocol::buffer::{ProtocolBuffer, ProtocolError};

fn fill_buffer<const N: usize>(buf: &mut ProtocolBuffer<N>, filled: usize) {
    buf.reset();
    buf.set_end(filled);
    buf.reset_position();
}

#[test]
fn test_consume_within_range() {
    let mut buffer = ProtocolBuffer::<64>::new();
    fill_buffer(&mut buffer, 10);

    assert_eq!(buffer.consume(4), Ok(false));
    assert_eq!(buffer.remaining(), 6);

    assert_eq!(buffer.consume(6), Ok(true));
    assert_eq!(buffer.remaining(), 0);
}

// remaining() is a uint16_t subtraction, so a position past the end reports a length bounded by the
// type rather than by the buffer, and move_unused() hands that length straight to memmove().
#[test]
fn test_consume_past_end_is_rejected() {
    let mut buffer = ProtocolBuffer::<64>::new();
    fill_buffer(&mut buffer, 10);

    assert!(matches!(
        buffer.consume(16),
        Err(ProtocolError::OutOfRangeConsume(16))
    ));
    assert!(buffer.remaining() <= buffer.reserved());
}

#[test]
fn test_consume_negative_is_rejected() {
    let mut buffer = ProtocolBuffer::<64>::new();
    fill_buffer(&mut buffer, 10);

    assert_eq!(buffer.consume(4), Ok(false));
    assert!(matches!(
        buffer.consume(-8),
        Err(ProtocolError::OutOfRangeConsume(-8))
    ));
    assert!(buffer.remaining() <= buffer.reserved());
}
