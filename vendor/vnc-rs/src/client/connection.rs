use futures::TryStreamExt;
use tokio_stream::wrappers::ReceiverStream;

use std::{future::Future, sync::Arc, vec};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{
        mpsc::{channel, error::TryRecvError, Receiver, Sender},
        oneshot, Mutex, OwnedSemaphorePermit, Semaphore,
    },
};
use tokio_util::compat::*;
use tracing::*;

use crate::{codec, PixelFormat, Rect, VncEncoding, VncError, VncEvent, X11Event};
const CHANNEL_SIZE: usize = 256;
const OUTPUT_EVENTS: usize = 4096;
const OUTPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
struct QueuedEvent {
    event: VncEvent,
    _permit: OwnedSemaphorePermit,
}

async fn enqueue_event(
    output: &Sender<QueuedEvent>,
    budget: &Arc<Semaphore>,
    event: VncEvent,
) -> Result<(), VncError> {
    let bytes = match &event {
        VncEvent::RawImage(_, data)
        | VncEvent::JpegImage(_, data)
        | VncEvent::SetCursor(_, data) => data.len(),
        VncEvent::Text(text) | VncEvent::Error(text) => text.len(),
        _ => 0,
    };
    if bytes > OUTPUT_BYTES {
        return Err(VncError::General(
            "VNC event exceeds output byte budget".into(),
        ));
    }
    let permit = budget
        .clone()
        .acquire_many_owned(bytes as u32)
        .await
        .map_err(|e| VncError::General(e.to_string()))?;
    output
        .send(QueuedEvent {
            event,
            _permit: permit,
        })
        .await
        .map_err(|e| VncError::General(e.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

use super::messages::{ClientMsg, ServerMsg};

struct ImageRect {
    rect: Rect,
    encoding: VncEncoding,
}

fn desktop_resize_result(reason: u16, status: u16) -> Option<VncEvent> {
    match status {
        0 => None,
        // RFB ExtendedDesktopSize status 4 means "request forwarded", not
        // rejection. A later server-side layout event may complete it.
        4 => Some(VncEvent::DesktopResizePending { reason }),
        _ => Some(VncEvent::DesktopResizeRejected { reason, status }),
    }
}

impl From<[u8; 12]> for ImageRect {
    fn from(buf: [u8; 12]) -> Self {
        Self {
            rect: Rect {
                x: ((buf[0] as u16) << 8) | buf[1] as u16,
                y: ((buf[2] as u16) << 8) | buf[3] as u16,
                width: ((buf[4] as u16) << 8) | buf[5] as u16,
                height: ((buf[6] as u16) << 8) | buf[7] as u16,
            },
            encoding: (((buf[8] as u32) << 24)
                | ((buf[9] as u32) << 16)
                | ((buf[10] as u32) << 8)
                | (buf[11] as u32))
                .into(),
        }
    }
}

impl ImageRect {
    async fn read<S>(reader: &mut S) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut rect_buf = [0_u8; 12];
        reader.read_exact(&mut rect_buf).await?;
        Ok(rect_buf.into())
    }
}

struct VncInner {
    name: String,
    screen: (u16, u16),
    input_ch: Sender<ClientMsg>,
    output_ch: Receiver<QueuedEvent>,
    decoding_stop: Option<oneshot::Sender<()>>,
    net_conn_stop: Option<oneshot::Sender<()>>,
    closed: bool,
}

/// The instance of a connected vnc client
///
impl VncInner {
    async fn new<S>(
        mut stream: S,
        shared: bool,
        mut pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (conn_ch_tx, conn_ch_rx) = channel(32);
        let (input_ch_tx, input_ch_rx) = channel(CHANNEL_SIZE);
        let (output_ch_tx, output_ch_rx) = channel(OUTPUT_EVENTS);
        let output_budget = Arc::new(Semaphore::new(OUTPUT_BYTES));
        let (decoding_stop_tx, decoding_stop_rx) = oneshot::channel();
        let (net_conn_stop_tx, net_conn_stop_rx) = oneshot::channel();

        trace!("client init msg");
        send_client_init(&mut stream, shared).await?;

        trace!("server init msg");
        let (name, (width, height)) =
            read_server_init(&mut stream, &mut pixel_format, &|e| async {
                enqueue_event(&output_ch_tx, &output_budget, e).await
            })
            .await?;

        trace!("client encodings: {:?}", encodings);
        send_client_encoding(&mut stream, encodings).await?;

        trace!("Require the first frame");
        input_ch_tx
            .send(ClientMsg::FramebufferUpdateRequest(
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                0,
            ))
            .await?;

        // start the decoding thread
        spawn(async move {
            trace!("Decoding thread starts");
            let mut conn_ch_rx = {
                let conn_ch_rx = ReceiverStream::new(conn_ch_rx).into_async_read();
                FuturesAsyncReadCompatExt::compat(conn_ch_rx)
            };

            let output_func = |e| async { enqueue_event(&output_ch_tx, &output_budget, e).await };

            let pf = pixel_format.as_ref().unwrap();
            if let Err(e) =
                asycn_vnc_read_loop(&mut conn_ch_rx, pf, &output_func, decoding_stop_rx).await
            {
                if let VncError::IoError(e) = e {
                    if let std::io::ErrorKind::UnexpectedEof = e.kind() {
                        // this should be a normal case when the network connection disconnects
                        // and we just send an EOF over the inner bridge between the process thread and the decode thread
                        // do nothing here
                    } else {
                        error!("Error occurs during the decoding {:?}", e);
                        let _ = output_func(VncEvent::Error(e.to_string())).await;
                    }
                } else {
                    error!("Error occurs during the decoding {:?}", e);
                    let _ = output_func(VncEvent::Error(e.to_string())).await;
                }
            }
            trace!("Decoding thread stops");
        });

        // start the traffic process thread
        spawn(async move {
            trace!("Net Connection thread starts");
            let _ =
                async_connection_process_loop(stream, input_ch_rx, conn_ch_tx, net_conn_stop_rx)
                    .await;
            trace!("Net Connection thread stops");
        });

        info!("VNC Client {name} starts");
        Ok(Self {
            name,
            screen: (width, height),
            input_ch: input_ch_tx,
            output_ch: output_ch_rx,
            decoding_stop: Some(decoding_stop_tx),
            net_conn_stop: Some(net_conn_stop_tx),
            closed: false,
        })
    }

    async fn input(&mut self, event: X11Event) -> Result<(), VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            let msg = match event {
                X11Event::Refresh => ClientMsg::FramebufferUpdateRequest(
                    Rect {
                        x: 0,
                        y: 0,
                        width: self.screen.0,
                        height: self.screen.1,
                    },
                    1,
                ),
                X11Event::FullRefresh => ClientMsg::FramebufferUpdateRequest(
                    Rect {
                        x: 0,
                        y: 0,
                        width: self.screen.0,
                        height: self.screen.1,
                    },
                    0, // non-incremental: server sends entire framebuffer
                ),
                X11Event::SetDesktopSize(screen) => {
                    ClientMsg::SetDesktopSize(screen.id, screen.width, screen.height, screen.flags)
                }
                X11Event::KeyEvent(key) => ClientMsg::KeyEvent(key.keycode, key.down),
                X11Event::ExtendedKeyEvent {
                    keysym,
                    keycode,
                    down,
                } => ClientMsg::ExtendedKeyEvent(keysym, keycode, down),
                X11Event::PointerEvent(mouse) => {
                    ClientMsg::PointerEvent(mouse.position_x, mouse.position_y, mouse.bottons)
                }
                X11Event::CopyText(text) => {
                    if text.len() > 1024 * 1024 {
                        return Err(VncError::General("VNC clipboard text exceeds 1 MiB".into()));
                    }
                    ClientMsg::ClientCutText(text)
                }
            };
            self.input_ch
                .try_send(msg)
                .map_err(|e| VncError::General(format!("VNC input queue unavailable: {e}")))?;
            Ok(())
        }
    }

    async fn recv_event(&mut self) -> Result<VncEvent, VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            match self.output_ch.recv().await {
                Some(e) => {
                    let e = e.event;
                    match &e {
                        VncEvent::SetResolution(s) => self.screen = (s.width, s.height),
                        VncEvent::DesktopResizeAvailable(s) => self.screen = (s.width, s.height),
                        _ => {}
                    }
                    Ok(e)
                }
                None => {
                    self.closed = true;
                    Err(VncError::ClientNotRunning)
                }
            }
        }
    }

    async fn poll_event(&mut self) -> Result<Option<VncEvent>, VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            match self.output_ch.try_recv() {
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    Err(VncError::ClientNotRunning)
                }
                Err(TryRecvError::Empty) => Ok(None),
                Ok(e) => {
                    let e = e.event;
                    match &e {
                        VncEvent::SetResolution(s) => self.screen = (s.width, s.height),
                        VncEvent::DesktopResizeAvailable(s) => self.screen = (s.width, s.height),
                        _ => {}
                    }
                    Ok(Some(e))
                }
            }
            // Ok(self.output_ch.recv().await)
        }
    }

    /// Stop the VNC engine and release resources
    ///
    fn close(&mut self) -> Result<(), VncError> {
        if self.net_conn_stop.is_some() {
            let net_conn_stop: oneshot::Sender<()> = self.net_conn_stop.take().unwrap();
            let _ = net_conn_stop.send(());
        }
        if self.decoding_stop.is_some() {
            let decoding_stop = self.decoding_stop.take().unwrap();
            let _ = decoding_stop.send(());
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for VncInner {
    fn drop(&mut self) {
        info!("VNC Client {} stops", self.name);
        let _ = self.close();
    }
}

pub struct VncClient {
    inner: Arc<Mutex<VncInner>>,
}

impl VncClient {
    /// Start ServerInit and decoding after the caller completes RFB security.
    /// The caller must authenticate the stream before invoking this constructor.
    pub async fn from_authenticated_stream<S>(
        stream: S,
        shared: bool,
        pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::new(stream, shared, pixel_format, encodings).await
    }

    pub(super) async fn new<S>(
        stream: S,
        shared: bool,
        pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Ok(Self {
            inner: Arc::new(Mutex::new(
                VncInner::new(stream, shared, pixel_format, encodings).await?,
            )),
        })
    }

    /// Input a `X11Event` from the frontend
    ///
    pub async fn input(&self, event: X11Event) -> Result<(), VncError> {
        self.inner.lock().await.input(event).await
    }

    /// Receive a `VncEvent` from the engine
    /// This function will block until a `VncEvent` is received
    ///
    pub async fn recv_event(&self) -> Result<VncEvent, VncError> {
        self.inner.lock().await.recv_event().await
    }

    /// polling `VncEvent` from the engine and give it to the client
    ///
    pub async fn poll_event(&self) -> Result<Option<VncEvent>, VncError> {
        self.inner.lock().await.poll_event().await
    }

    /// Stop the VNC engine and release resources
    ///
    pub async fn close(&self) -> Result<(), VncError> {
        self.inner.lock().await.close()
    }
}

impl Clone for VncClient {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

async fn send_client_init<S>(stream: &mut S, shared: bool) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    trace!("Send shared flag: {}", shared);
    stream.write_u8(shared as u8).await?;
    Ok(())
}

async fn read_server_init<S, F, Fut>(
    stream: &mut S,
    pf: &mut Option<PixelFormat>,
    output_func: &F,
) -> Result<(String, (u16, u16)), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    // +--------------+--------------+------------------------------+
    // | No. of bytes | Type [Value] | Description                  |
    // +--------------+--------------+------------------------------+
    // | 2            | U16          | framebuffer-width in pixels  |
    // | 2            | U16          | framebuffer-height in pixels |
    // | 16           | PIXEL_FORMAT | server-pixel-format          |
    // | 4            | U32          | name-length                  |
    // | name-length  | U8 array     | name-string                  |
    // +--------------+--------------+------------------------------+

    let screen_width = stream.read_u16().await?;
    let screen_height = stream.read_u16().await?;
    let mut send_our_pf = false;

    output_func(VncEvent::SetResolution(
        (screen_width, screen_height).into(),
    ))
    .await?;

    let pixel_format = PixelFormat::read(stream).await?;
    if pf.is_none() {
        output_func(VncEvent::SetPixelFormat(pixel_format)).await?;
        let _ = pf.insert(pixel_format);
    } else {
        send_our_pf = true;
    }

    let name_len = stream.read_u32().await?;
    let mut name_buf = vec![0_u8; name_len as usize];
    stream.read_exact(&mut name_buf).await?;
    let name = String::from_utf8_lossy(&name_buf).into_owned();

    if send_our_pf {
        trace!("Send customized pixel format {:#?}", pf);
        ClientMsg::SetPixelFormat(*pf.as_ref().unwrap())
            .write(stream)
            .await?;
    }
    Ok((name, (screen_width, screen_height)))
}

async fn send_client_encoding<S>(
    stream: &mut S,
    encodings: Vec<VncEncoding>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    ClientMsg::SetEncodings(encodings).write(stream).await?;
    Ok(())
}

async fn asycn_vnc_read_loop<S, F, Fut>(
    stream: &mut S,
    pf: &PixelFormat,
    output_func: &F,
    mut stop_ch: oneshot::Receiver<()>,
) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    let mut raw_decoder = codec::RawDecoder::new();
    let mut zrle_decoder = codec::ZrleDecoder::new();
    let mut tight_decoder = codec::TightDecoder::new();
    let mut trle_decoder = codec::TrleDecoder::new();
    let mut cursor = codec::CursorDecoder::new();

    // main decoding loop
    while let Err(oneshot::error::TryRecvError::Empty) = stop_ch.try_recv() {
        let server_msg = ServerMsg::read(stream).await?;
        trace!("Server message got: {:?}", server_msg);
        match server_msg {
            ServerMsg::FramebufferUpdate(rect_num) => {
                for _ in 0..rect_num {
                    let rect = ImageRect::read(stream).await?;
                    // trace!("Encoding: {:?}", rect.encoding);

                    match rect.encoding {
                        VncEncoding::Raw => {
                            raw_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::CopyRect => {
                            let source_x = stream.read_u16().await?;
                            let source_y = stream.read_u16().await?;
                            let mut src_rect = rect.rect;
                            src_rect.x = source_x;
                            src_rect.y = source_y;
                            output_func(VncEvent::Copy(rect.rect, src_rect)).await?;
                        }
                        VncEncoding::Tight => {
                            tight_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::Trle => {
                            trle_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::Zrle => {
                            zrle_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::CursorPseudo => {
                            cursor.decode(pf, &rect.rect, stream, output_func).await?;
                        }
                        VncEncoding::QemuExtendedKeyEventPseudo => {
                            output_func(VncEvent::ExtendedKeyEventAvailable).await?;
                        }
                        VncEncoding::DesktopSizePseudo => {
                            output_func(VncEvent::SetResolution(
                                (rect.rect.width, rect.rect.height).into(),
                            ))
                            .await?;
                        }
                        VncEncoding::ExtendedDesktopSizePseudo => {
                            let screens = stream.read_u8().await? as usize;
                            let mut padding = [0u8; 3];
                            stream.read_exact(&mut padding).await?;
                            let mut primary_screen = None;
                            for index in 0..screens {
                                let mut screen = [0u8; 16];
                                stream.read_exact(&mut screen).await?;
                                if index == 0 {
                                    primary_screen = Some((
                                        u32::from_be_bytes(screen[..4].try_into().unwrap()),
                                        u32::from_be_bytes(screen[12..16].try_into().unwrap()),
                                    ));
                                }
                            }
                            if rect.rect.y == 0 {
                                if let Some((id, flags)) = primary_screen {
                                    output_func(VncEvent::DesktopResizeAvailable(
                                        crate::DesktopScreen {
                                            id,
                                            width: rect.rect.width,
                                            height: rect.rect.height,
                                            flags,
                                        },
                                    ))
                                    .await?;
                                }
                            } else if let Some(event) =
                                desktop_resize_result(rect.rect.x, rect.rect.y)
                            {
                                output_func(event).await?;
                            }
                        }
                        VncEncoding::LastRectPseudo => {
                            break;
                        }
                    }
                }
                output_func(VncEvent::FramebufferUpdated).await?;
            }
            // SetColorMapEntries,
            ServerMsg::Bell => {
                output_func(VncEvent::Bell).await?;
            }
            ServerMsg::ServerCutText(text) => {
                output_func(VncEvent::Text(text)).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod fjern_resize_tests {
    use super::*;

    #[tokio::test]
    async fn outgoing_key_edges_are_ordered_and_eof_ends_transport() {
        let (client, mut server) = tokio::io::duplex(64);
        let (input, input_rx) = channel(2);
        let (output, _output_rx) = channel(2);
        let (_stop, stop_rx) = oneshot::channel();
        input.send(ClientMsg::KeyEvent(65, true)).await.unwrap();
        input.send(ClientMsg::KeyEvent(65, false)).await.unwrap();
        let task = tokio::spawn(async_connection_process_loop(
            client, input_rx, output, stop_rx,
        ));
        let mut wire = [0; 16];
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            server.read_exact(&mut wire),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(wire, [4, 1, 0, 0, 0, 0, 0, 65, 4, 0, 0, 0, 0, 0, 0, 65]);
        drop(server);
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn permanently_stalled_write_has_a_deadline() {
        let (client, _server) = tokio::io::duplex(1);
        let (input, input_rx) = channel(2);
        let (output, _output_rx) = channel(2);
        let (_stop, stop_rx) = oneshot::channel();
        input.send(ClientMsg::KeyEvent(65, true)).await.unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(7),
            async_connection_process_loop(client, input_rx, output, stop_rx),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("write timed out"));
    }

    #[tokio::test]
    async fn output_queue_bounds_bytes_without_throttling_small_tiles() {
        let (tx, mut rx) = channel(OUTPUT_EVENTS);
        let budget = Arc::new(Semaphore::new(OUTPUT_BYTES));
        let rect = Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        for _ in 0..2040 {
            enqueue_event(&tx, &budget, VncEvent::RawImage(rect, vec![0; 64 * 64 * 4]))
                .await
                .unwrap();
        }
        assert_eq!(rx.len(), 2040);
        while let Ok(event) = rx.try_recv() {
            drop(event);
        }
        assert_eq!(budget.available_permits(), OUTPUT_BYTES);

        let budget = Arc::new(Semaphore::new(16));
        enqueue_event(&tx, &budget, VncEvent::RawImage(rect, vec![0; 16]))
            .await
            .unwrap();
        let waiting = enqueue_event(&tx, &budget, VncEvent::RawImage(rect, vec![1; 1]));
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        drop(rx.recv().await.unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut waiting)
            .await
            .unwrap()
            .unwrap();
        drop(rx.recv().await.unwrap());
        assert_eq!(budget.available_permits(), 16);
        assert!(enqueue_event(
            &tx,
            &budget,
            VncEvent::RawImage(rect, vec![0; OUTPUT_BYTES + 1])
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn extended_resize_updates_both_receive_paths_and_refresh_wire() {
        for polling in [false, true] {
            let (tx, rx) = channel(2);
            let (input, mut messages) = channel(2);
            let mut inner = VncInner {
                name: String::new(),
                screen: (64, 64),
                input_ch: input,
                output_ch: rx,
                decoding_stop: None,
                net_conn_stop: None,
                closed: false,
            };
            enqueue_event(
                &tx,
                &Arc::new(Semaphore::new(OUTPUT_BYTES)),
                VncEvent::DesktopResizeAvailable(crate::DesktopScreen {
                    id: 1,
                    width: 1920,
                    height: 1080,
                    flags: 0,
                }),
            )
            .await
            .unwrap();
            if polling {
                inner.poll_event().await.unwrap();
            } else {
                inner.recv_event().await.unwrap();
            }
            for event in [X11Event::Refresh, X11Event::FullRefresh] {
                inner.input(event).await.unwrap();
                let mut wire = Vec::new();
                messages
                    .recv()
                    .await
                    .unwrap()
                    .write(&mut wire)
                    .await
                    .unwrap();
                assert_eq!(&wire[6..10], &[7, 128, 4, 56]);
            }
            inner.input(X11Event::Refresh).await.unwrap();
            inner.input(X11Event::Refresh).await.unwrap();
            assert!(inner.input(X11Event::Refresh).await.is_err());
        }
    }

    #[tokio::test]
    async fn full_decoder_queue_resumes_without_input_and_stop_is_prompt() {
        let (client, mut server) = tokio::io::duplex(64);
        let (_input, input_rx) = channel(1);
        let (output, mut output_rx) = channel(1);
        output.send(Ok(vec![9])).await.unwrap();
        let (stop, stop_rx) = oneshot::channel();
        let task = tokio::spawn(async_connection_process_loop(
            client, input_rx, output, stop_rx,
        ));
        server.write_all(&[1, 2, 3]).await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(output_rx.recv().await.unwrap().unwrap(), [9]);
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(bytes, [1, 2, 3]);
        stop.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn stalled_write_does_not_block_reads_or_shutdown() {
        let (client, mut server) = tokio::io::duplex(16);
        let (input, input_rx) = channel(2);
        let (output, mut output_rx) = channel(2);
        let (stop, stop_rx) = oneshot::channel();
        input
            .send(ClientMsg::ClientCutText("x".repeat(1024)))
            .await
            .unwrap();
        let task = tokio::spawn(async_connection_process_loop(
            client, input_rx, output, stop_rx,
        ));
        server.write_all(&[42]).await.unwrap();
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(bytes, [42]);
        stop.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn forwarded_resize_is_pending_not_rejected() {
        assert!(matches!(
            desktop_resize_result(1, 4),
            Some(VncEvent::DesktopResizePending { reason: 1 })
        ));
        assert!(matches!(
            desktop_resize_result(1, 3),
            Some(VncEvent::DesktopResizeRejected {
                reason: 1,
                status: 3
            })
        ));
        assert!(desktop_resize_result(1, 0).is_none());
    }
}

async fn async_connection_process_loop<S>(
    stream: S,
    mut input_ch: Receiver<ClientMsg>,
    conn_ch: Sender<std::io::Result<Vec<u8>>>,
    mut stop_ch: oneshot::Receiver<()>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let receive = async {
        loop {
            // Capacity wakes this task directly. Reserve before reading so no
            // payload is copied or lost while the decoder is backpressured.
            let permit = conn_ch
                .reserve()
                .await
                .map_err(|_| VncError::ClientNotRunning)?;
            let mut buffer = vec![0; 65536];
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                return Ok::<(), VncError>(());
            }
            buffer.truncate(n);
            permit.send(Ok(buffer));
        }
    };
    let transmit = async {
        while let Some(msg) = input_ch.recv().await {
            tokio::time::timeout(std::time::Duration::from_secs(5), msg.write(&mut writer))
                .await
                .map_err(|_| VncError::General("VNC write timed out".into()))??;
        }
        Ok::<(), VncError>(())
    };
    // Reads continue during a slow write. Cancellation drops both halves;
    // never resume a partially written RFB message.
    tokio::select! {
        _ = &mut stop_ch => Ok(()),
        result = receive => result,
        result = transmit => result,
    }
}
