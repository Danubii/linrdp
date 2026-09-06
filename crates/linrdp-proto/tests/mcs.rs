use linrdp_proto::{
    mcs::{self, Settings},
    negotiation::SecurityProtocol,
};

// Independently assembled TLS-only server settings response using MS-RDPBCGR
// 4.1.4 layout, with zero virtual channels and protocol echo 0x0b.
const RESPONSE: &[u8] = &[
    0x7f, 0x66, 0x5a, 0x0a, 1, 0, 2, 1, 0, 0x30, 0x1a, 2, 1, 0x22, 2, 1, 3, 2, 1, 0, 2, 1, 1, 2, 1,
    0, 2, 1, 1, 2, 3, 0, 0xff, 0xf8, 2, 1, 2, 4, 0x36, 0, 5, 0, 20, 124, 0, 1, 0x2a, 0x14, 0x76,
    0x0a, 1, 1, 0, 1, 0xc0, 0, b'M', b'c', b'D', b'n', 32, 1, 0x0c, 12, 0, 4, 0, 8, 0, 11, 0, 0, 0,
    3, 0x0c, 8, 0, 0xeb, 3, 0, 0, 2, 0x0c, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[test]
fn decodes_independent_connect_response() {
    let result = mcs::connect_response(RESPONSE, 11).unwrap();
    assert_eq!(result.version, 0x80004);
    assert_eq!(result.io_channel, 1003);
    assert_eq!(result.early_capability_flags, 0);
    assert!(mcs::connect_response(RESPONSE, 2).is_err());
}

#[test]
fn rejects_truncated_and_malformed_responses_without_panicking() {
    for end in 0..RESPONSE.len() {
        assert!(
            mcs::connect_response(&RESPONSE[..end], 11).is_err(),
            "length {end}"
        );
    }
    let mut extra = RESPONSE.to_vec();
    extra.push(0);
    assert!(mcs::connect_response(&extra, 11).is_err());
    // Outer tag, outer length, MCS result, domain tag, OID, key and data length.
    for index in [0, 1, 2, 5, 9, 43, 59, 62] {
        let mut bad = RESPONSE.to_vec();
        bad[index] ^= 0x40;
        assert!(mcs::connect_response(&bad, 11).is_err(), "offset {index}");
    }
    // Exercise every single-byte mutation for panic freedom.
    for index in 0..RESPONSE.len() {
        for value in 0..=255 {
            let mut packet = RESPONSE.to_vec();
            packet[index] = value;
            let _ = mcs::connect_response(&packet, 11);
        }
    }
}

#[test]
fn rejects_security_changes_duplicate_blocks_and_unrequested_channels() {
    for (offset, value) in [
        (RESPONSE.len() - 8, 1),
        (RESPONSE.len() - 4, 1),
        (79, 1),
        (77, 0),
    ] {
        let mut bad = RESPONSE.to_vec();
        bad[offset] = value;
        assert!(mcs::connect_response(&bad, 11).is_err(), "offset {offset}");
    }
    let mut duplicate = RESPONSE.to_vec();
    // Replace network block with a second core block of invalid size.
    duplicate[73] = 1;
    assert!(mcs::connect_response(&duplicate, 11).is_err());
}

#[test]
fn channel_messages_match_published_examples_and_bind_confirmations() {
    assert_eq!(mcs::attach_confirm(&[0x2e, 0, 0, 6]).unwrap(), 1007);
    assert_eq!(
        mcs::join_request(1007, 1003).unwrap(),
        [0x38, 0, 6, 3, 0xeb]
    );
    let confirm = [0x3e, 0, 0, 6, 3, 0xeb, 3, 0xeb];
    mcs::join_confirm(&confirm, 1007, 1003).unwrap();
    assert!(mcs::join_confirm(&confirm, 1008, 1003).is_err());
    assert!(mcs::join_confirm(&confirm, 1007, 1004).is_err());
    assert!(mcs::attach_confirm(&[0x2e, 0, 0xff, 0xff]).is_err());
    assert!(mcs::attach_confirm(&[0x2e, 0x20, 0, 6]).is_err());
    assert!(mcs::join_request(1000, 1003).is_err());
    for end in 0..confirm.len() {
        assert!(mcs::join_confirm(&confirm[..end], 1007, 1003).is_err());
    }
}

#[test]
fn initial_settings_encode_fixed_profile_and_selected_security() {
    for (protocol, selected) in [
        (SecurityProtocol::Tls, 1),
        (SecurityProtocol::CredSsp, 2),
        (SecurityProtocol::CredSspEarlyAuth, 8),
    ] {
        let packet = mcs::connect_initial(Settings::default(), protocol).unwrap();
        assert_eq!(&packet[..2], &[0x7f, 0x65]);
        let core = packet
            .windows(4)
            .position(|v| v == [1, 0xc0, 216, 0])
            .unwrap();
        assert_eq!(&packet[core + 8..core + 12], &[0, 4, 0, 3]);
        assert_eq!(&packet[core + 140..core + 146], &[16, 0, 2, 0, 0, 0]);
        assert_eq!(&packet[core + 212..core + 216], &[selected, 0, 0, 0]);
        assert_eq!(
            &packet[core + 216..],
            &[
                2, 0xc0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0xc0, 8, 0, 0, 0, 0, 0
            ]
        );
        assert!(packet.len() < 1024);
    }
    assert!(
        mcs::connect_initial(
            Settings {
                width: 0,
                ..Settings::default()
            },
            SecurityProtocol::Tls
        )
        .is_err()
    );
}
