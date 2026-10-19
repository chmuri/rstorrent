use rt_rpc::{RpcError, RpcManager, MAX_CONTENT_SIZE, MIN_SIZE_LIMIT};

#[test]
fn test_size_limit_bounds() {
    let mut manager = RpcManager::new();

    assert!(matches!(
        manager.set_size_limit(0),
        Err(RpcError::InputError(_))
    ));
    assert!(matches!(
        manager.set_size_limit(MIN_SIZE_LIMIT - 1),
        Err(RpcError::InputError(_))
    ));
    assert!(matches!(
        manager.set_size_limit(MAX_CONTENT_SIZE + 1),
        Err(RpcError::InputError(_))
    ));

    assert!(manager.set_size_limit(MIN_SIZE_LIMIT).is_ok());
    assert_eq!(manager.size_limit(), MIN_SIZE_LIMIT);

    assert!(manager.set_size_limit(MAX_CONTENT_SIZE).is_ok());
    assert_eq!(manager.size_limit(), MAX_CONTENT_SIZE);
}
