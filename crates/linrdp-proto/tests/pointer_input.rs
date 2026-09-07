use linrdp_proto::desktop::{Input, Phase, Session};

fn active() -> Session {
    let mut session = Session::new(1004, 1003).unwrap();
    session.phase = Phase::Active;
    session
}

fn left(down: bool, x: u16, y: u16) -> Input {
    Input::Button {
        button: 1,
        down,
        x,
        y,
    }
}

#[test]
fn fast_double_click_keeps_all_four_edges_and_their_coordinates_in_one_pdu() {
    let mut session = active();
    let packet = session
        .input(&[
            left(true, 20, 30),
            left(false, 20, 30),
            left(true, 21, 31),
            left(false, 21, 31),
        ])
        .unwrap()
        .unwrap();
    // Independent MS-RDPBCGR 2.2.8.1.1.3 / 2.2.8.1.1.3.1.1.3 wire vector:
    // four TS_INPUT_EVENT records, INPUT_EVENT_MOUSE=0x8001,
    // PTRFLAGS_BUTTON1=0x1000 and PTRFLAGS_DOWN=0x8000 only on down edges.
    // eventTime is ignored by the server, so no synthetic timing is inserted.
    assert_eq!(
        packet,
        [
            0x64, 0, 3, 3, 0xeb, 0x70, 70, // MCS request and payload length
            70, 0, 0x17, 0, 0xec, 3, // Share Control: Data, source1004
            0, 0, 0, 0, 0, 1, 52, 0, 28, 0, 0, 0, // Share Data: Input, no compression
            4, 0, 0, 0, // numberEvents and padding
            0, 0, 0, 0, 1, 0x80, 0, 0x90, 20, 0, 30, 0, 0, 0, 0, 0, 1, 0x80, 0, 0x10, 20, 0, 30, 0,
            0, 0, 0, 0, 1, 0x80, 0, 0x90, 21, 0, 31, 0, 0, 0, 0, 0, 1, 0x80, 0, 0x10, 21, 0, 31, 0,
        ]
    );
    assert!(session.input(&[Input::ReleaseAll]).unwrap().is_none());
}

#[test]
fn double_click_crosses_batch_boundaries_and_focus_release_uses_latest_position() {
    let mut session = active();
    let first = session.input(&[left(true, 100, 200)]).unwrap().unwrap();
    assert_eq!(
        &first[first.len() - 12..],
        &[0, 0, 0, 0, 1, 0x80, 0, 0x90, 100, 0, 200, 0]
    );
    let second = session
        .input(&[
            left(false, 100, 200),
            left(true, 102, 201),
            Input::Move { x: 104, y: 202 },
            Input::ReleaseAll,
        ])
        .unwrap()
        .unwrap();
    assert_eq!(
        &second[second.len() - 52..],
        &[
            4, 0, 0, 0, 0, 0, 0, 0, 1, 0x80, 0, 0x10, 100, 0, 200, 0, 0, 0, 0, 0, 1, 0x80, 0, 0x90,
            102, 0, 201, 0, 0, 0, 0, 0, 1, 0x80, 0, 8, 104, 0, 202, 0, 0, 0, 0, 0, 1, 0x80, 0,
            0x10, 104, 0, 202, 0,
        ]
    );
    // The physical release arriving after focus-loss cleanup must not add a
    // second up event or affect the next complete click.
    assert!(session.input(&[left(false, 104, 202)]).unwrap().is_none());
    let next = session
        .input(&[left(true, 104, 202), left(false, 104, 202)])
        .unwrap()
        .unwrap();
    assert_eq!(&next[next.len() - 28..next.len() - 24], &[2, 0, 0, 0]);
}
