//! The client's link to mooring: the socket, the in-flight user action, background
//! syncs and the recognition status the host pushes. Views never call `rpc` themselves.
use crate::connection::{RecognitionConnection, StatusLink};
use fibo_core::delivery::{PATIENCE, Settled, deliver};
use fibo_core::{model::*, protocol::*};
use futures::channel::mpsc::UnboundedSender;
use std::{
    collections::VecDeque,
    future::Future,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A host restart drops the status link for a moment and it reconnects on its own;
/// only an outage longer than this is worth telling the user about.
pub const CUT_OFF_NOTICE: Duration = Duration::from_secs(3);
/// A new terminal's slot: whether it splits, its task, the lost pane it replaces.
pub type Launch = (bool, Option<String>, Option<String>);

pub struct Store {
    socket: PathBuf,
    /// A user action is in flight; further actions wait for it.
    busy: bool,
    /// The in-flight user action has taken long enough to show 「正在更新…」.
    slow: bool,
    busy_seq: u64,
    /// A background refresh is in flight; it never blocks input or shows a label.
    syncing: bool,
    sync_again: bool,
    /// The user action in flight, when it is a request (launches are not).
    in_flight: Option<Request>,
    /// Requests made while another user action was in flight, in order.
    queued: VecDeque<Request>,
    /// The launch asked for while another action was in flight; it runs after `queued`.
    held_launch: Option<Launch>,
    recognition: RecognitionStatus,
    /// When the status link to the host dropped, and why; `None` while it is up.
    cut_off_at: Option<(Instant, String)>,
    _recognition_connection: RecognitionConnection,
}

impl Store {
    pub fn new(socket: PathBuf, status: UnboundedSender<StatusLink>) -> Self {
        let recognition_connection = RecognitionConnection::new(&socket, status);
        Self {
            socket,
            busy: false,
            slow: false,
            busy_seq: 0,
            syncing: false,
            sync_again: false,
            in_flight: None,
            queued: VecDeque::new(),
            held_launch: None,
            recognition: RecognitionStatus::default(),
            cut_off_at: None,
            _recognition_connection: recognition_connection,
        }
    }
    pub fn socket(&self) -> &Path {
        &self.socket
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn slow(&self) -> bool {
        self.slow
    }
    pub fn recognition(&self) -> &RecognitionStatus {
        &self.recognition
    }
    /// A status from the host: the link is up, whatever phase it reports. A host can
    /// say recognition is Interrupted while every connection works; that is not a cut.
    pub fn set_recognition(&mut self, status: RecognitionStatus) {
        self.cut_off_at = None;
        self.recognition = status;
    }
    /// What the status subscription reported. The last status stays while the link is
    /// down; reconnecting brings a whole new one.
    pub fn take_link(&mut self, link: StatusLink) {
        match link {
            StatusLink::Status(status) => self.set_recognition(*status),
            StatusLink::Lost(error) => {
                if self.cut_off_at.is_none() {
                    self.cut_off_at = Some((Instant::now(), error));
                }
            }
        }
    }
    /// The status link has been down at least `CUT_OFF_NOTICE`.
    pub fn cut_off(&self) -> bool {
        self.cut_off_at
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= CUT_OFF_NOTICE)
    }
    /// The status link is down, however briefly.
    pub fn link_down(&self) -> bool {
        self.cut_off_at.is_some()
    }
    /// Why the status link dropped, while it is down.
    pub fn cut_off_reason(&self) -> Option<&str> {
        self.cut_off_at.as_ref().map(|(_, why)| why.as_str())
    }
    /// The host has published a newer workspace than the one at `revision`.
    pub fn behind(&self, revision: u64) -> bool {
        self.recognition.workspace_revision > revision
    }
    /// Marks a user action in flight and returns its sequence number.
    pub fn begin_busy(&mut self, request: Option<&Request>) -> u64 {
        self.busy = true;
        self.in_flight = request.cloned();
        self.busy_seq += 1;
        self.busy_seq
    }
    /// Shows 「正在更新…」 if action `seq` is still the one in flight.
    pub fn mark_slow(&mut self, seq: u64) -> bool {
        if self.busy && self.busy_seq == seq {
            self.slow = true;
            return true;
        }
        false
    }
    pub fn end_busy(&mut self) {
        self.busy = false;
        self.in_flight = None;
        self.slow = false;
    }
    /// A request made during another action waits its turn. Dropped only when it repeats
    /// one still pending: a second creation of the same kind (a double click makes one,
    /// not two) or a second refresh.
    pub fn hold(&mut self, request: Request) {
        let key = repeat_key(&request);
        let repeats = |r: &Request| key.is_some() && repeat_key(r) == key;
        if self.in_flight.as_ref().is_some_and(repeats) || self.queued.iter().any(repeats) {
            return;
        }
        self.queued.push_back(request);
    }
    pub fn next_queued(&mut self) -> Option<Request> {
        self.queued.pop_front()
    }
    /// Whether a launch may start now; during another action it waits instead, the
    /// latest one asked for (a double click opens one terminal, not two).
    pub fn admit_launch(
        &mut self,
        split: bool,
        task: &Option<String>,
        replace: &Option<String>,
    ) -> bool {
        if self.busy {
            self.held_launch = Some((split, task.clone(), replace.clone()));
        }
        !self.busy
    }
    pub fn next_launch(&mut self) -> Option<Launch> {
        self.held_launch.take()
    }
    /// Starts a background sync unless one is in flight; that one fetches again after.
    pub fn begin_sync(&mut self) -> bool {
        if self.syncing {
            self.sync_again = true;
            return false;
        }
        self.syncing = true;
        self.sync_again = false;
        true
    }
    /// Ends the sync in flight; true when another was asked for meanwhile.
    pub fn end_sync(&mut self) -> bool {
        self.syncing = false;
        std::mem::take(&mut self.sync_again)
    }
    pub fn send(&self, request: Request) -> impl Future<Output = io::Result<Reply>> + use<> {
        let socket = self.socket.clone();
        async move { rpc(&socket, &request) }
    }
    /// A user's request: resent with the same id while the host is away, until it is
    /// confirmed, refused, or `PATIENCE` says its outcome is unknown.
    pub fn deliver(&self, request: Request) -> impl Future<Output = Settled> + use<> {
        let socket = self.socket.clone();
        async move { deliver(&socket, &request, PATIENCE) }
    }
    /// Launches a terminal, then places it (`place`: a new tab, a split, a relaunch in
    /// an old pane's place), each resent with the same id while the host is away. A split from a running
    /// shell first asks the host for that shell's current directory. Given `spaces`, a
    /// new tab whose directory lies in a space inside its own goes to that space.
    pub fn launch(
        &self,
        cwd_source: Option<String>,
        spaces: Option<Vec<Space>>,
        mut request: Request,
        mut place: Edit,
    ) -> impl Future<Output = io::Result<Reply>> + use<> {
        let socket = self.socket.clone();
        async move {
            // A shell that ended or moved somewhere unreadable keeps the recorded directory.
            if let Some(id) = cwd_source
                && let Ok(Reply::Cwd(current)) = rpc(&socket, &Request::TerminalCwd { id })
                // An attached host's folders are on its machine, and it just read this one.
                && (crate::lifecycle::attached() || Path::new(&current).is_dir())
                && let Request::Launch { cwd, .. } = &mut request
            {
                *cwd = current;
            }
            if let Some(spaces) = spaces
                && let Request::Launch { space, cwd, .. } = &mut request
                && let Some(narrower) = narrower_space(&spaces, space, cwd)
                && let Edit::CreateTab { tab } = &mut place
            {
                space.clone_from(&narrower.id);
                tab.space.clone_from(&narrower.id);
            }
            let Reply::Workspace(_) = deliver(&socket, &request, PATIENCE).into_result()? else {
                return Err(std::io::Error::other("launch response missing"));
            };
            let place = Request::Edit {
                request: Some(RequestId::fresh()),
                edit: place,
            };
            deliver(&socket, &place, PATIENCE).into_result()
        }
    }
}

pub fn record_left(socket: &Path, at: u64) {
    if let Err(e) = rpc(socket, &Request::ClientLeft { at }) {
        eprintln!("记录离开时间失败: {e}");
    }
}
/// What makes two pending requests the same click: creations by kind, a refresh.
pub fn repeat_key(request: &Request) -> Option<String> {
    match request {
        Request::Edit { edit, .. } => match edit {
            Edit::AddTask(_) => Some("add task".into()),
            Edit::AddSpace(_) => Some("add space".into()),
            Edit::SaveSessionTask { session, .. } => Some(format!("save {session}")),
            _ => None,
        },
        Request::Workspace => Some("workspace".into()),
        _ => None,
    }
}
