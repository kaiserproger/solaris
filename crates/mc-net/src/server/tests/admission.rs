use super::super::*;
use super::support::*;

#[test]
fn public_bind_detection_is_conservative() {
    assert!(!is_public_bind("127.0.0.1:25565".parse().unwrap()));
    assert!(!is_public_bind("10.0.0.1:25565".parse().unwrap()));
    assert!(!is_public_bind("192.168.1.5:25565".parse().unwrap()));
    assert!(!is_public_bind("[::ffff:127.0.0.1]:25565".parse().unwrap()));
    assert!(is_public_bind("0.0.0.0:25565".parse().unwrap()));
    assert!(is_public_bind("8.8.8.8:25565".parse().unwrap()));
}

#[test]
fn public_security_rejects_local_dev_operator_fallback() {
    let permissions = CommandPermissionConfig::new(Vec::<String>::new(), true).with_login_access(
        login::LoginAccessConfig::normalized(
            true,
            false,
            std::iter::empty::<&str>(),
            std::iter::empty::<&str>(),
        ),
    );
    let err = validate_public_security_config("8.8.8.8:25565".parse().unwrap(), &permissions)
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    assert!(err.to_string().contains("allow_local_dev_operators"));
}

#[test]
fn public_security_allows_online_mode() {
    let permissions = CommandPermissionConfig::new(["Notch"], false).with_login_access(
        login::LoginAccessConfig::normalized(
            true,
            false,
            std::iter::empty::<&str>(),
            std::iter::empty::<&str>(),
        ),
    );
    validate_public_security_config("8.8.8.8:25565".parse().unwrap(), &permissions).unwrap();
}

#[test]
fn private_security_allows_local_offline_dev() {
    validate_public_security_config(
        "127.0.0.1:25565".parse().unwrap(),
        &CommandPermissionConfig::new(Vec::<String>::new(), true),
    )
    .unwrap();
}

/// Console-shaped control handle over one permission config.

#[test]
fn console_operator_grant_applies_without_restart() {
    let profile = login::LoggedInProfile {
        uuid: uuid::Uuid::from_u128(7),
        name: "Builder".into(),
    };
    let uuid = profile.uuid.to_string();
    let config = CommandPermissionConfig::new(Vec::<String>::new(), false);
    let control = access_control_handle(&config);
    let peer = "192.168.1.20:40000".parse().unwrap();

    // A fresh login and an online session both start without authority.
    assert!(!config.permissions_for(&profile, peer).is_op());
    assert!(!config.live_permissions_for("Builder", &uuid, peer).is_op());

    assert_eq!(control.set_operator(" Builder ", true), ["builder"]);

    // The next login resolves through the live set, and an online session
    // picks it up on its next command.
    assert!(config.permissions_for(&profile, peer).is_op());
    assert!(config.live_permissions_for("builder", &uuid, peer).is_op());
    assert!(config.live_permissions_for("Builder", &uuid, peer).is_op());

    control.set_operator("builder", false);
    assert!(!config.permissions_for(&profile, peer).is_op());
    // A session that logged in while listed must lose that authority too.
    assert!(!config.live_permissions_for("Builder", &uuid, peer).is_op());
}

#[test]
fn console_operator_grant_retires_loopback_dev_fallback() {
    let config = CommandPermissionConfig::new(Vec::<String>::new(), true);
    let control = access_control_handle(&config);
    let peer = "127.0.0.1:40000".parse().unwrap();
    let dev = login::LoggedInProfile {
        uuid: uuid::Uuid::from_u128(9),
        name: "Dev".into(),
    };
    let uuid = dev.uuid.to_string();

    assert!(config.permissions_for(&dev, peer).is_op());
    control.set_operator("Builder", true);
    // Configuring any operator retires the empty-list fallback, matching
    // what a fresh login decides.
    assert!(!config.permissions_for(&dev, peer).is_op());
    assert!(!config.live_permissions_for("Dev", &uuid, peer).is_op());
    control.set_operator("Builder", false);
    assert!(config.live_permissions_for("Dev", &uuid, peer).is_op());
}

#[test]
fn console_whitelist_entry_applies_to_next_login() {
    let config = CommandPermissionConfig::new(Vec::<String>::new(), false).with_login_access(
        login::LoginAccessConfig::normalized(
            false,
            true,
            Vec::<String>::new(),
            Vec::<String>::new(),
        ),
    );
    let control = access_control_handle(&config);
    let uuid = uuid::Uuid::from_u128(11);
    let rejected = Some(login::LoginRejection::Whitelist);

    assert_eq!(
        login::access_rejection(config.login_access(), "Builder", uuid),
        rejected
    );

    assert_eq!(control.set_whitelisted("Builder", true), ["builder"]);
    assert_eq!(
        login::access_rejection(config.login_access(), "Builder", uuid),
        None
    );
    assert_eq!(
        login::access_rejection(config.login_access(), "Stranger", uuid),
        rejected
    );

    control.set_whitelisted("builder", false);
    assert_eq!(
        login::access_rejection(config.login_access(), "Builder", uuid),
        rejected
    );
}

#[test]
fn local_dev_operator_fallback_requires_loopback_peer() {
    let profile = login::LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "LanPlayer".into(),
    };
    let fallback = CommandPermissionConfig::new(Vec::<String>::new(), true);

    assert_eq!(
        fallback.permissions_for(&profile, "127.0.0.1:40000".parse().unwrap()),
        play::commands::CommandPermissions::from_op(true)
    );
    assert_eq!(
        fallback.permissions_for(&profile, "[::ffff:127.0.0.1]:40000".parse().unwrap()),
        play::commands::CommandPermissions::from_op(true)
    );
    assert_eq!(
        fallback.permissions_for(&profile, "192.168.1.20:40000".parse().unwrap()),
        play::commands::CommandPermissions::from_op(false)
    );

    let explicit = CommandPermissionConfig::new(["  LanPlayer  ", "  "], true);
    assert_eq!(
        explicit.permissions_for(&profile, "192.168.1.20:40000".parse().unwrap()),
        play::commands::CommandPermissions::from_op(true)
    );
}
