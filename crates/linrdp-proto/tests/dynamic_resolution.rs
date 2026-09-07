use linrdp_proto::{display_control::DisplayControl, mcs, negotiation::SecurityProtocol};

// Independently assemble the BER/GCC response, including SC_NET's odd-count padding.
fn response(channels: &[u16]) -> Vec<u8> {
    let mut blocks = vec![1, 12, 12, 0, 4, 0, 8, 0, 11, 0, 0, 0];
    blocks.extend([3, 12]);
    let network_len = 8 + channels.len() * 2 + (channels.len() % 2) * 2;
    blocks.extend((network_len as u16).to_le_bytes());
    blocks.extend(1003u16.to_le_bytes());
    blocks.extend((channels.len() as u16).to_le_bytes());
    for channel in channels {
        blocks.extend(channel.to_le_bytes());
    }
    if channels.len() % 2 == 1 {
        blocks.extend([0, 0]);
    }
    blocks.extend([2, 12, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut gcc = vec![0, 5, 0, 20, 124, 0, 1, 0, 20, 0x76, 10, 1, 1, 0, 1, 0xc0, 0];
    gcc.extend(b"McDn");
    gcc.push(blocks.len() as u8);
    gcc.extend(blocks);
    let mut body = vec![10, 1, 0, 2, 1, 0, 0x30, 24];
    for _ in 0..8 {
        body.extend([2, 1, 1]);
    }
    body.extend([4, gcc.len() as u8]);
    body.extend(gcc);
    let mut packet = vec![0x7f, 0x66, body.len() as u8];
    packet.extend(body);
    packet
}

#[test]
fn static_channel_assignments_preserve_request_order() {
    for (clipboard, dynamic_resolution, names, channels) in [
        (false, false, vec![], vec![]),
        (true, false, vec![b"cliprdr\0"], vec![1005]),
        (false, true, vec![b"drdynvc\0"], vec![1006]),
        (
            true,
            true,
            vec![b"cliprdr\0", b"drdynvc\0"],
            vec![1005, 1006],
        ),
    ] {
        let settings = mcs::Settings {
            clipboard,
            dynamic_resolution,
            ..Default::default()
        };
        let packet = mcs::connect_initial(settings, SecurityProtocol::CredSsp).unwrap();
        if dynamic_resolution {
            let core = packet
                .windows(4)
                .position(|bytes| bytes == [1, 0xc0, 234, 0])
                .unwrap();
            assert_eq!(packet[core + 144] & 0x40, 0x40);
            assert_eq!(&packet[core + 216..core + 226], &[0; 10]);
            assert_eq!(
                &packet[core + 226..core + 234],
                &[100, 0, 0, 0, 100, 0, 0, 0]
            );
        }

        let offset = packet
            .windows(2)
            .rposition(|bytes| bytes == [3, 0xc0])
            .unwrap();
        let network = &packet[offset..];
        assert_eq!(
            u16::from_le_bytes(network[2..4].try_into().unwrap()) as usize,
            network.len()
        );
        assert_eq!(
            u32::from_le_bytes(network[4..8].try_into().unwrap()) as usize,
            names.len()
        );
        for (entry, name) in network[8..].as_chunks::<12>().0.iter().zip(names) {
            assert_eq!(&entry[..8], name);
            assert_eq!(&entry[8..], &[0, 0, 0, 0xc0]);
        }
        let server = mcs::connect_response(&response(&channels), 11).unwrap();
        assert_eq!(
            server.static_channels,
            [channels.first().copied(), channels.get(1).copied()]
        );
    }
}

#[test]
fn channel_assignment_rejects_collisions_invalid_ids_and_truncation() {
    for channels in [
        &[1003][..],
        &[1000],
        &[1005, 1005],
        &[1005, 1003],
        &[1005, 1006, 1007],
    ] {
        assert!(mcs::connect_response(&response(channels), 11).is_err());
    }
    for channels in [&[][..], &[1005], &[1005, 1006]] {
        let packet = response(channels);
        for end in 0..packet.len() {
            assert!(mcs::connect_response(&packet[..end], 11).is_err());
        }
    }
}

fn open() -> DisplayControl {
    let mut state = DisplayControl::default();
    assert_eq!(
        state
            .receive(&[0x50, 0, 2, 0, 1, 0, 2, 0, 3, 0, 4, 0])
            .unwrap(),
        vec![vec![0x50, 0, 2, 0]]
    );
    // A four-byte channel ID exercises a different encoding than unit tests.
    let mut create = vec![0x12, 0, 0, 1, 0];
    create.extend(b"Microsoft::Windows::RDS::DisplayControl\0");
    assert_eq!(
        state.receive(&create).unwrap(),
        vec![vec![0x12, 0, 0, 1, 0, 0, 0, 0, 0]]
    );
    state
}

fn caps() -> Vec<u8> {
    [5u32, 20, 1, 800, 600]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

#[test]
fn fragmented_capabilities_enforce_area_and_close_revokes_layout() {
    let mut state = open();
    assert!(state.layout(800, 600).is_err());
    let capabilities = caps();
    let mut first = vec![0x26, 0, 0, 1, 0, 20, 0];
    first.extend(&capabilities[..9]);
    state.receive(&first).unwrap();
    assert!(state.partial());
    assert!(!state.ready());
    let mut last = vec![0x32, 0, 0, 1, 0];
    last.extend(&capabilities[9..]);
    state.receive(&last).unwrap();
    assert!(!state.partial());
    assert_eq!(state.size(801, 600), Some((800, 600)));
    assert!(state.layout(802, 600).is_err());
    assert!(state.layout(800, 600).is_ok());
    assert_eq!(
        state.receive(&[0x42, 0, 0, 1, 0]).unwrap(),
        vec![vec![0x42, 0, 0, 1, 0]]
    );
    assert!(!state.ready());
    assert!(state.layout(800, 600).is_err());
}

#[test]
fn malformed_capabilities_and_fragments_are_rejected() {
    for packet in [
        &[0x50, 0, 0, 0][..],
        &[0x50, 0, 4, 0],
        &[0x50, 1, 1, 0],
        &[0x50, 0, 2, 0],
    ] {
        assert!(DisplayControl::default().receive(packet).is_err());
    }
    for packet in [
        &[0x22, 0, 0, 1, 0, 21, 0][..],
        &[0x2e, 0, 0, 1, 0, 20],
        &[0x32, 1, 0, 1, 0],
        &[0x42, 0, 0, 1, 0, 0],
    ] {
        assert!(open().receive(packet).is_err());
    }
    for field in [0, 1, 2, 3, 4] {
        let mut packet = vec![0x32, 0, 0, 1, 0];
        let mut invalid = caps();
        invalid[field * 4..field * 4 + 4].fill(0);
        packet.extend(invalid);
        assert!(open().receive(&packet).is_err());
    }
    let mut state = open();
    let first = [0x22, 0, 0, 1, 0, 20, 5];
    state.receive(&first).unwrap();
    assert!(state.receive(&first).is_err());
}

#[test]
fn dvc_caps_and_create_responses_do_not_forward_static_headers_to_server_endpoint() {
    use linrdp_proto::channel;
    let caps = [0x50, 0, 2, 0];
    assert_eq!(
        channel::send_dvc(1004, 1005, &caps).unwrap(),
        vec![vec![
            0x64, 0, 3, 3, 0xed, 0x70, 12, 4, 0, 0, 0, 3, 0, 0, 0, 0x50, 0, 2, 0,
        ]]
    );
    let create = [0x10, 3, 0, 0, 0, 0];
    assert_eq!(
        channel::send_dvc(1004, 1005, &create).unwrap(),
        vec![vec![
            0x64, 0, 3, 3, 0xed, 0x70, 14, 6, 0, 0, 0, 3, 0, 0, 0, 0x10, 3, 0, 0, 0, 0,
        ]]
    );
    // Clipboard retains its own endpoint's existing SHOW_PROTOCOL behavior.
    assert_eq!(channel::send(1004, 1005, &caps).unwrap()[0][11], 0x13);
    let large = channel::send_dvc(1004, 1005, &[0; 1600]).unwrap();
    assert_eq!(&large[0][6..8], &[0x86, 0x48]);
    assert_eq!(&large[0][12..16], &[3, 0, 0, 0]);
    assert!(channel::send_dvc(1004, 1005, &[0; 1601]).is_err());
}
