//! P3 inventory/storage acceptance fixture: one trade action at a time.
//!
//! `mode = "inventory-storage"` runs this module. A player runs the package's
//! `trade` root with exactly one action as its argument, and the fixture answers
//! one atomic transaction of that player's inventory and this package's own
//! storage:
//!
//! - `buy` reads the ledger under `trade-ledger`, asks for two emeralds to leave
//!   the inventory and one apple to enter it, and swaps the ledger to the
//!   purchased count plus one at the version it just read.
//! - `refund` asks for the opposite inventory change and swaps the ledger to the
//!   count minus one.
//! - `retry-original` submits the first purchase's own content again: the same
//!   two emeralds and one apple, `expected-version: none`, and the ledger value
//!   `1` that purchase wrote. Once the first purchase committed, that swap can no
//!   longer match, so the whole transaction is refused with the player's funds
//!   still there - which is what makes it a probe of the storage side rather than
//!   of the inventory side.
//! - `stale-session` submits a refund's content against the session the first
//!   trade command named, which a reconnect has left behind: the owners refuse it
//!   and the live session's inventory and storage stay exactly as they were.
//! - `wide` is the one action outside the acceptance sequence, for the host's
//!   staging bound: it is a purchase whose storage side also writes three values
//!   at the contract's largest single length, so the record's text is far past one
//!   string's bound while no string is past its own.
//!
//! Five more actions ask for a server-owned menu instead of a transaction. Each
//! one is a fire-and-forget command - the contract answers a menu command with
//! nothing at all - so the fixture reports only that it staged the request
//! (`P3_MENU <action>-requested`) and the server's own wire frames and click
//! routing are what establish the effect:
//!
//! - `menu` opens the market on the connection that asked, `menu-next` opens the
//!   second market, and `menu-close` closes the market this fixture holds.
//! - `stale-menu-close` asks to close the first market while the second one is the
//!   one open: the server's window matches plugin, player and menu together, so a
//!   close naming another menu is not one it applies.
//! - `stale-session-menu` asks for the market and for a close of the second one on
//!   the connection the first command named - which a reconnect has left behind -
//!   and reports its marker to the live connection, so a test reads that the
//!   request was made where it is and nothing happened where it was addressed.
//!
//! One click on an open menu of this fixture is a trade action of its own. Slot 0
//! buys with a primary or shift-primary click and refunds with a secondary or
//! shift-secondary one, and slot 8 closes the menu. The fixture closes the menu it
//! is holding before any transaction it starts - the same order the shipped
//! economy plugin uses - so a click can never name a window whose transaction is
//! already in flight. It opens nothing afterwards: a click that traded, committed
//! or refused, and a click on the close button all leave the connection with no
//! menu of this fixture's, so a client that wants another action asks for another
//! open. The server publishes a click only to the plugin that opened the menu, and
//! only for the window, revision and menu it still holds, so a stale click never
//! reaches this callback at all.
//!
//! Every transaction reuses the correlation id `trade-txn`. The contract records
//! no durable operation for this command, so that id is correlation only: the
//! fixture reuses it precisely because reading it as a receipt would be reading a
//! promise the server never made.
//!
//! One request is outstanding at a time. After each action's typed answer the
//! fixture reads the ledger back, checks exactly what that answer implies - a
//! commit at the value it asked for and a version past the one it named, or a
//! refusal with both sides exactly as they were read - and only then reports
//! `P3_TRADE <action> <committed|refused> ledger=<count>` to the player. A
//! mismatch is reported as `P3_TRADE_UNEXPECTED <action> step <n>: <detail>` on
//! the operator log instead, and never as a marker: the host drops the batch of a
//! callback that answered with a failure, so a diagnostic here is a log line and
//! cannot be a chat line.
//!
//! The fixture also records the leave of the session it traded on - `P3_TRADE_LEFT
//! <session>` on the operator log - because `stale-session` is only the contract's
//! own probe once the server has finished with that connection, and a driver that
//! has to reconnect cannot know that from its own side.

use solaris_plugin_sdk::events::{CommandInvoked, InventoryMenuClicked, InventoryStorageOutcome};
use solaris_plugin_sdk::{
    close_inventory_menu, inventory_storage_transaction, log, message_player, open_inventory_menu,
    storage, storage_get, Command, Event, Failure, InventoryClick, InventoryMenuSlot,
    InventoryResourceDelta, LogLevel, StorageGetOutcome, StorageMutation, StorageRecord,
};

/// The command root the package's manifest declares for this fixture.
pub(crate) const ROOT: &str = "trade";
/// The correlation id every transaction of this fixture names. It is not a
/// durable operation id: the contract records no receipt for this command, and a
/// later action reuses it deliberately.
const REQUEST: &str = "trade-txn";
/// The key the purchased count lives under: a decimal count, absent meaning zero.
const LEDGER_KEY: &str = "trade-ledger";
/// The count the first purchase writes, which is also the value the
/// `retry-original` probe submits again.
const FIRST_PURCHASE: &str = "1";
/// The item one purchase removes from the player's inventory.
const EMERALD: &str = "minecraft:emerald";
/// The item one purchase adds to it.
const APPLE: &str = "minecraft:apple";
/// How many emeralds one purchase costs.
const EMERALD_COST: i16 = -2;
/// How many apples one purchase grants.
const APPLE_GRANT: i16 = 1;
/// The menu the fixture's market is: the id the server's window matches a close
/// against, and the title the client is shown.
const MARKET: &str = "trade-market";
const MARKET_TITLE: &str = "WASM Market";
/// The second menu, which `stale-menu-close` keeps open while it names the first
/// one and `stale-session-menu` asks a dead connection to close.
const NEXT_MARKET: &str = "trade-market-next";
const NEXT_MARKET_TITLE: &str = "WASM Market Next";
/// The one button that buys and refunds, and the one that closes: a server-owned
/// menu is a row of fixed buttons, and the contract states these two.
const BUY_SLOT: u8 = 0;
const CLOSE_SLOT: u8 = 8;
/// The item the close button shows. It is a real item of the server's own
/// registry, so the menu the owner opens carries it like any other.
const BARRIER: &str = "minecraft:barrier";
/// What the two buttons say. A test reads them back from the content frame the
/// server publishes, so they are the fixture's own wording and not a value the
/// server derived.
const APPLE_LABEL: &str = "Apple: buy 2 emeralds / refund 1 apple";
const CLOSE_LABEL: &str = "Close";
/// The keys the opt-in `wide` action writes beside the ledger.
const WIDE_KEYS: [&str; 3] = ["trade-wide-1", "trade-wide-2", "trade-wide-3"];
/// How long each of those values is: the contract's own largest storage value
/// (`mc_script::MAX_PLUGIN_STORAGE_VALUE_BYTES`). Three of them are past one
/// returned string's bound while each is inside its own, which is the staging
/// boundary `wide` exists to exercise.
const WIDE_VALUE_BYTES: usize = 4_096;

/// One request a player can ask the fixture for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Requested {
    /// One menu request: it concludes in the callback that asks for it, because
    /// the contract answers a menu command with nothing at all.
    Menu(MenuAction),
    /// One trade action: a state machine over two ledger reads and one
    /// transaction.
    Trade(Action),
}

impl Requested {
    /// The request one argument names, or nothing when it names none of them.
    fn from_name(name: &str) -> Option<Self> {
        if let Some(menu) = MenuAction::from_name(name) {
            return Some(Self::Menu(menu));
        }
        Action::from_name(name).map(Self::Trade)
    }
}

/// One menu request a player can ask the fixture for.
///
/// Every one of them is a fire-and-forget command: nothing in this contract
/// answers a menu command, so no request here can learn that a menu was opened or
/// closed. What the fixture records is therefore what it asked for - marked
/// `requested`, never `applied` - and the server's own frames, its own click
/// routing and the owner's own window are what establish the effect.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    /// Open the market on the connection that asked.
    Open,
    /// Open the second market on the connection that asked.
    Next,
    /// Close the market this fixture holds.
    Close,
    /// Ask to close the first market while the second one is the one open. The
    /// server matches a close against the window it holds - plugin, player and
    /// menu together - so a close naming another menu is not one it applies.
    StaleClose,
    /// Ask for the market, and for a close of the second one, on the connection
    /// the first command named: a reconnect has left it behind, so the server
    /// drops both instead of reaching the player's later connection.
    StaleSession,
}

impl MenuAction {
    /// The request one argument names, or nothing when it names no request.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "menu" => Some(Self::Open),
            "menu-next" => Some(Self::Next),
            "menu-close" => Some(Self::Close),
            "stale-menu-close" => Some(Self::StaleClose),
            "stale-session-menu" => Some(Self::StaleSession),
            _ => None,
        }
    }

    /// The line the fixture reports once the request is staged.
    ///
    /// Requested is not an acknowledgement: the host admits the batch and hands
    /// the commands to the owners, and whether the owner opened, closed or ignored
    /// anything is the owner's own business, which this contract does not answer
    /// back to the plugin.
    fn marker(self) -> &'static str {
        match self {
            Self::Open => "P3_MENU open-requested",
            Self::Next => "P3_MENU next-open-requested",
            Self::Close => "P3_MENU close-requested",
            Self::StaleClose => "P3_MENU stale-close-requested",
            Self::StaleSession => "P3_MENU stale-session-requested",
        }
    }
}

/// One menu this fixture asked the server to open and has not closed since: the
/// id it named, and the connection it asked on.
///
/// A click on that menu carries the connection too, and the fixture acts on the
/// connection the event names rather than on this one; what this holds is the
/// answer to the other question a click asks - whether the menu it names is one
/// this fixture is serving at all - and the connection a command-driven close has
/// to name.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Active {
    id: &'static str,
    session: u64,
}

/// One trade action a player can ask the fixture for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// One purchase: two emeralds out, one apple in, the ledger up one.
    Buy,
    /// One refund: the opposite inventory change, the ledger down one.
    Refund,
    /// The first purchase's own content, submitted again once it committed.
    RetryOriginal,
    /// A refund addressed to the session the first trade command named.
    StaleSession,
    /// A purchase whose storage side carries three values at the contract's
    /// largest single length, for the host's staging bound.
    Wide,
}

/// How one action asks the ledger to move.
enum Swap {
    /// Swap against the version just read, to the observed count plus this amount.
    Observed(i64),
    /// The first purchase's own request: the ledger is expected to hold nothing at
    /// all, and the value asked for is the one that purchase wrote.
    Original,
}

impl Action {
    /// The action one argument names, or nothing when it names no action.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "buy" => Some(Self::Buy),
            "refund" => Some(Self::Refund),
            "retry-original" => Some(Self::RetryOriginal),
            "stale-session" => Some(Self::StaleSession),
            "wide" => Some(Self::Wide),
            _ => None,
        }
    }

    /// The action's own name, exactly as the command argument and the marker line
    /// spell it.
    fn name(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Refund => "refund",
            Self::RetryOriginal => "retry-original",
            Self::StaleSession => "stale-session",
            Self::Wide => "wide",
        }
    }

    /// The prefix of this action's two correlation ids. Distinct per action, so an
    /// answer that does not belong to the read this fixture is waiting on is
    /// refused instead of being read as that read's result.
    fn request(self) -> &'static str {
        match self {
            Self::Buy => "trade-buy",
            Self::Refund => "trade-refund",
            Self::RetryOriginal => "trade-retry-original",
            Self::StaleSession => "trade-stale-session",
            Self::Wide => "trade-wide",
        }
    }

    /// The inventory change one action asks for, as resource and delta.
    fn inventory(self) -> [(&'static str, i16); 2] {
        match self {
            Self::Buy | Self::Wide | Self::RetryOriginal => {
                [(EMERALD, EMERALD_COST), (APPLE, APPLE_GRANT)]
            }
            Self::Refund | Self::StaleSession => [(EMERALD, -EMERALD_COST), (APPLE, -APPLE_GRANT)],
        }
    }

    /// The ledger swap one action asks for.
    fn swap(self) -> Swap {
        match self {
            Self::Buy | Self::Wide => Swap::Observed(1),
            Self::Refund | Self::StaleSession => Swap::Observed(-1),
            Self::RetryOriginal => Swap::Original,
        }
    }
}

/// One of the two reads an action makes of the ledger.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    /// The read that decides what the action asks for.
    Decide,
    /// The read that checks what the owners did with it.
    Verify,
}

impl Read {
    /// The word this read is named by inside the request id it sends.
    fn name(self) -> &'static str {
        match self {
            Self::Decide => "before",
            Self::Verify => "after",
        }
    }
}

/// Where one action stands: which answer the fixture is waiting for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing outstanding: the fixture is between actions.
    Idle,
    /// The read that decides what the action asks for.
    Decide,
    /// The owners' typed answer to the transaction.
    Decided,
    /// The read that checks what the owners did.
    Verify,
}

impl Step {
    /// The number a diagnostic line names this step by.
    fn number(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Decide => 1,
            Self::Decided => 2,
            Self::Verify => 3,
        }
    }
}

/// The fixture itself: the action in flight and everything it read.
pub struct Trade {
    /// The session the first trade command named, which the stale-session probe
    /// addresses. Captured once and never replaced: a later command's session is
    /// the connection that replaced it.
    first_session: Option<u64>,
    /// Whether the server has announced that session leaving. Until it has, the
    /// stale-session probe would be addressing a connection the server may still
    /// hold, so the fixture refuses to run it.
    first_left: bool,
    /// The menu this fixture asked the server to open and has not closed since.
    menu: Option<Active>,
    /// The player the action in flight reports to, from the command that began it.
    reporter: Option<String>,
    /// The action in flight, if any.
    action: Option<Action>,
    /// The session the action's transaction names.
    session: u64,
    /// What the fixture is waiting for.
    step: Step,
    /// The ledger the action read before its transaction, exactly as answered.
    before: Option<StorageRecord>,
    /// The value the action's swap asks the ledger to hold.
    expected: String,
    /// The owners' typed answer to the transaction, before the read-back confirms
    /// it.
    outcome: Option<InventoryStorageOutcome>,
}

impl Trade {
    /// The fixture before any trade command: nothing outstanding and no session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            first_session: None,
            first_left: false,
            menu: None,
            reporter: None,
            action: None,
            session: 0,
            step: Step::Idle,
            before: None,
            expected: String::new(),
            outcome: None,
        }
    }

    /// One delivered batch: the commands the player ran, and at most one answer to
    /// this fixture's outstanding request.
    ///
    /// `delegated` names the `trade` actions another fixture of this deployment
    /// answers. This state machine neither answers one of them nor reports it as an
    /// action the root does not have: the fixture that owns the name has already
    /// seen the command, and everything else about a batch is unchanged.
    pub fn on_events(
        &mut self,
        events: &[Event],
        delegated: &[&str],
    ) -> Result<Vec<Command>, Failure> {
        let mut answers = events.iter().filter(|event| is_answer(event));
        let first = answers.next();
        if answers.next().is_some() {
            return Err(self.reject("two answers arrived for one outstanding request"));
        }
        let mut commands = Vec::new();
        if let Some(event) = first {
            commands.extend(self.answer(event)?);
        }
        for event in events {
            match event {
                Event::CommandInvoked(invoked) => {
                    if invoked.name == ROOT {
                        commands.extend(self.begin(invoked, delegated)?);
                    }
                }
                // One click on a menu this fixture opened. The server publishes a
                // click only to the plugin that opened that menu, and only while it
                // still holds the window and the revision it was clicked at, so a
                // stale click never arrives here at all: what reaches this arm is a
                // click on a window this plugin owns right now.
                Event::InventoryMenuClicked(clicked) => {
                    commands.extend(self.clicked(clicked)?);
                }
                // The connection the fixture traded on has gone. The server drops a
                // session before it announces the leave, so this line is also the
                // point after which the same player can connect again - which is
                // what a driver that has to reconnect waits for.
                Event::PlayerLeft(left) => {
                    if self.first_session == Some(left.session) {
                        self.first_left = true;
                        log(LogLevel::Debug, &format!("P3_TRADE_LEFT {}", left.session));
                    }
                }
                _ => {}
            }
        }
        Ok(commands)
    }

    /// One `trade` command: what it names, and - for a trade action - the read
    /// that decides what that action asks for. Nothing about a trade is changed
    /// here: the transaction is built from the server's own answer to that read.
    /// A menu request is answered on the spot, because the contract answers a menu
    /// command with nothing to wait for.
    ///
    /// An action another fixture of this deployment owns is not this state
    /// machine's to answer or to report, and leaves it exactly as it was.
    fn begin(
        &mut self,
        invoked: &CommandInvoked,
        delegated: &[&str],
    ) -> Result<Vec<Command>, Failure> {
        let named = invoked.arguments.first().map(String::as_str);
        if named.is_some_and(|name| delegated.contains(&name)) {
            return Ok(Vec::new());
        }
        if self.action.is_some() {
            return Err(self.reject("an action was asked for while another was in flight"));
        }
        let Some(requested) = named.and_then(Requested::from_name) else {
            return Err(self.reject(&format!(
                "the trade root was asked for without one of its actions: {:?}",
                invoked.arguments
            )));
        };
        if self.first_session.is_none() {
            self.first_session = Some(invoked.session);
        }
        match requested {
            Requested::Menu(menu) => self.request_menu(menu, invoked),
            Requested::Trade(action) => self.begin_trade(action, invoked.session, &invoked.player),
        }
    }

    /// One menu request, staged against the state this fixture holds.
    ///
    /// The command is built for the connection the request names - the one that
    /// asked, except for the stale probe, which names the connection the first
    /// command saw - and the marker goes to the player who asked, which is the
    /// live connection. Nothing here assumes the owner applied anything.
    fn request_menu(
        &mut self,
        menu: MenuAction,
        invoked: &CommandInvoked,
    ) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        match menu {
            MenuAction::Open | MenuAction::Next => {
                let (id, title) = if menu == MenuAction::Open {
                    (MARKET, MARKET_TITLE)
                } else {
                    (NEXT_MARKET, NEXT_MARKET_TITLE)
                };
                // One open replaces whatever this fixture held: the server
                // allocates a window for every open, and the id recorded here is
                // the one a later click has to match.
                self.menu = Some(Active {
                    id,
                    session: invoked.session,
                });
                commands.push(open_inventory_menu(
                    invoked.session,
                    id,
                    title,
                    market_slots(),
                ));
            }
            MenuAction::Close => {
                let Some(active) = self.menu.take() else {
                    return Err(self.reject("a close was asked for with no menu open"));
                };
                commands.push(close_inventory_menu(active.session, active.id));
            }
            MenuAction::StaleClose => {
                let Some(active) = self.menu else {
                    return Err(self.reject("a stale close was asked for with no menu open"));
                };
                if active.id == MARKET {
                    // The probe is only the contract's own case while the menu it
                    // names is not the one open: closing the open menu would be an
                    // ordinary close.
                    return Err(
                        self.reject("the stale close names the menu this fixture is holding")
                    );
                }
                commands.push(close_inventory_menu(active.session, MARKET));
            }
            MenuAction::StaleSession => {
                let Some(first) = self.first_session else {
                    return Err(self.reject("no command named a session to address"));
                };
                if first == invoked.session {
                    // The probe addresses the connection the first command named,
                    // so it is only the contract's own case on a later connection.
                    return Err(self.reject("the live session is the first one this fixture saw"));
                }
                if !self.first_left {
                    // A menu is owned by one connection, so the probe is only the
                    // contract's own case once the server has announced that
                    // connection leaving. Asking earlier would reach whatever the
                    // server still holds under that id, which is the opposite of
                    // what this action is for.
                    return Err(self.reject(&format!(
                        "the session the first command named ({first}) has not left yet"
                    )));
                }
                commands.push(open_inventory_menu(
                    first,
                    MARKET,
                    MARKET_TITLE,
                    market_slots(),
                ));
                commands.push(close_inventory_menu(first, NEXT_MARKET));
            }
        }
        commands.push(message_player(&invoked.player, menu.marker()));
        Ok(commands)
    }

    /// One click the server routed to this plugin.
    ///
    /// Slot 0 is the purchase button: a primary or shift-primary click buys,
    /// secondary or shift-secondary refunds, and the action then runs exactly as
    /// the same trade command would. Slot 8 closes. Any other slot, or a click for
    /// a menu this fixture is not holding, is nothing this fixture serves.
    fn clicked(&mut self, clicked: &InventoryMenuClicked) -> Result<Vec<Command>, Failure> {
        let held = self.menu.map(|active| active.id);
        if held != Some(clicked.menu.as_str()) {
            // The fixture holds no such menu, so there is nothing this click could
            // mean here. The server is what keeps such a click from reaching its
            // owner, so this is a diagnostic and never a marker.
            log(
                LogLevel::Debug,
                &format!(
                    "P3_MENU_IGNORED {} slot {} with {:?} open",
                    clicked.menu, clicked.slot, held
                ),
            );
            return Ok(Vec::new());
        }
        let active = self.menu.expect("the menu was just matched");
        if active.session != clicked.session {
            // The server publishes a click to the plugin that opened the window it
            // was clicked at, so the connection the event names is the connection
            // the fixture's own open command named. A different one means this
            // fixture is holding a window that is not the one the click came from,
            // and acting on it would be an effect on a connection the click never
            // reached.
            return Err(self.reject("the click names a connection this menu is not open on"));
        }
        if clicked.slot == CLOSE_SLOT {
            // The close button asks for the same thing the `menu-close` action
            // asks for, so it reports the same marker: a test reads the request,
            // and the server's own close frame is what shows it was applied.
            self.menu = None;
            return Ok(vec![
                close_inventory_menu(clicked.session, active.id),
                message_player(&clicked.player, MenuAction::Close.marker()),
            ]);
        }
        if clicked.slot != BUY_SLOT {
            log(
                LogLevel::Debug,
                &format!("P3_MENU_IGNORED {} slot {}", clicked.menu, clicked.slot),
            );
            return Ok(Vec::new());
        }
        if self.action.is_some() {
            // A click cannot arrive for a menu this fixture already closed, so a
            // second transaction would mean the fixture's own state is wrong.
            return Err(self.reject("a click arrived while an action was in flight"));
        }
        // The clicked action runs on the connection the click event carried - the
        // one the menu was opened on - and reports to the player it carried.
        self.begin_trade(
            clicked_action(clicked.click),
            clicked.session,
            &clicked.player,
        )
    }

    /// Begin a trade on its requested session after closing the tracked menu.
    /// The menu keeps its own session even when a stale trade targets a dead one.
    fn begin_trade(
        &mut self,
        action: Action,
        session: u64,
        player: &str,
    ) -> Result<Vec<Command>, Failure> {
        // The action's own preconditions are checked before anything is changed,
        // so a request the fixture refuses leaves its state exactly as it was.
        let session = match action {
            Action::StaleSession => {
                let Some(first) = self.first_session else {
                    return Err(self.reject("no trade command named a session to address"));
                };
                if first == session {
                    return Err(self.reject("the live session is the first one this fixture saw"));
                }
                if !self.first_left {
                    // A transaction is an effect on one live connection, so the
                    // probe is only the contract's own case once the server has
                    // announced that connection leaving. Running it earlier would
                    // address whatever the server still holds under that id, which
                    // is the opposite of what this action is for.
                    return Err(self.reject(&format!(
                        "the session the first trade command named ({first}) has not left yet"
                    )));
                }
                first
            }
            _ => session,
        };
        let mut commands = self.close_open_menu();
        self.reporter = Some(player.to_owned());
        self.action = Some(action);
        self.session = session;
        self.step = Step::Decide;
        self.before = None;
        self.outcome = None;
        let request = format!("{}-{}", action.request(), Read::Decide.name());
        commands.push(storage_get(&request, LEDGER_KEY));
        Ok(commands)
    }

    /// Close the tracked menu on the connection where it was opened.
    fn close_open_menu(&mut self) -> Vec<Command> {
        match self.menu.take() {
            Some(active) => vec![close_inventory_menu(active.session, active.id)],
            None => Vec::new(),
        }
    }

    /// Open the fixture's own market on one connection, replacing whatever this
    /// fixture held before.
    ///
    /// This is the same market a `menu` action asks for and it is tracked in the
    /// same place, so a click on it reaches the click arm above and a later close -
    /// from a command or from the zone the P3 fixture watches - names the
    /// connection that opened it.
    pub(crate) fn open_market(&mut self, session: u64) -> Vec<Command> {
        self.menu = Some(Active {
            id: MARKET,
            session,
        });
        vec![open_inventory_menu(
            session,
            MARKET,
            MARKET_TITLE,
            market_slots(),
        )]
    }

    /// Close the menu this fixture is holding, on the connection that opened it.
    /// Nothing is closed while it holds none.
    pub(crate) fn close_market(&mut self) -> Vec<Command> {
        self.close_open_menu()
    }

    /// One answer to this fixture's outstanding request.
    fn answer(&mut self, event: &Event) -> Result<Vec<Command>, Failure> {
        let Some(action) = self.action else {
            return Err(self.reject("an answer arrived with no request outstanding"));
        };
        match (self.step, event) {
            (Step::Decide, Event::StorageGetAnswered(answered)) => {
                let request = format!("{}-{}", action.request(), Read::Decide.name());
                if answered.request.as_str() != request.as_str() {
                    return Err(self.reject(&format!(
                        "the deciding read was answered under {}, not {request}",
                        answered.request
                    )));
                }
                match &answered.outcome {
                    StorageGetOutcome::Read(record) => self.transaction(action, record),
                    StorageGetOutcome::Failed(failure) => {
                        Err(self.reject(&format!("the ledger could not be read: {failure:?}")))
                    }
                }
            }
            (Step::Decided, Event::InventoryStorageTransactionAnswered(answered)) => {
                if answered.request.as_str() != REQUEST {
                    return Err(self.reject(&format!(
                        "the transaction was answered under {}, not {REQUEST}",
                        answered.request
                    )));
                }
                self.outcome = Some(answered.outcome.clone());
                self.step = Step::Verify;
                let request = format!("{}-{}", action.request(), Read::Verify.name());
                Ok(vec![storage_get(&request, LEDGER_KEY)])
            }
            (Step::Verify, Event::StorageGetAnswered(answered)) => {
                let request = format!("{}-{}", action.request(), Read::Verify.name());
                if answered.request.as_str() != request.as_str() {
                    return Err(self.reject(&format!(
                        "the read-back was answered under {}, not {request}",
                        answered.request
                    )));
                }
                match &answered.outcome {
                    StorageGetOutcome::Read(record) => self.conclude(action, record),
                    StorageGetOutcome::Failed(failure) => {
                        Err(self.reject(&format!("the ledger could not be read back: {failure:?}")))
                    }
                }
            }
            (_, event) => Err(self.reject(&format!("answered by {}", kind(event)))),
        }
    }

    /// The transaction one action submits, built from the ledger record its
    /// deciding read was answered with.
    fn transaction(
        &mut self,
        action: Action,
        record: &StorageRecord,
    ) -> Result<Vec<Command>, Failure> {
        let count = match count_of(record) {
            Ok(count) => count,
            Err(detail) => return Err(self.reject(&detail)),
        };
        if action == Action::RetryOriginal && record.value.as_deref() != Some(FIRST_PURCHASE) {
            // The probe submits the first purchase again, so it is only the
            // contract's own case when the ledger already holds what that purchase
            // wrote: `expected-version: none` is then refused by the key existing,
            // not by an amount the player cannot afford.
            return Err(self.reject(&format!(
                "retry-original expects the ledger at {FIRST_PURCHASE}, read {:?}",
                record.value
            )));
        }
        let (expected_version, value) = match action.swap() {
            Swap::Original => (None, FIRST_PURCHASE.to_owned()),
            Swap::Observed(delta) => (record.version, (count + delta).to_string()),
        };
        let mut mutations = vec![StorageMutation::Cas(storage::StorageCasMutation {
            key: LEDGER_KEY.to_owned(),
            expected_version,
            value: value.clone(),
        })];
        if action == Action::Wide {
            // Three more records, each at the contract's largest single value: the
            // batch's text is what a per-string bound must not be confused with.
            let wide = "w".repeat(WIDE_VALUE_BYTES);
            mutations.extend(WIDE_KEYS.iter().map(|key| {
                StorageMutation::Cas(storage::StorageCasMutation {
                    key: (*key).to_owned(),
                    expected_version: None,
                    value: wide.clone(),
                })
            }));
        }
        let inventory = action
            .inventory()
            .into_iter()
            .map(|(resource, delta)| InventoryResourceDelta {
                resource: resource.to_owned(),
                delta,
            })
            .collect();
        self.before = Some(record.clone());
        self.expected = value;
        self.step = Step::Decided;
        Ok(vec![inventory_storage_transaction(
            REQUEST,
            self.session,
            inventory,
            mutations,
        )])
    }

    /// Check the read-back against the answer the owners gave, and report the
    /// marker only when the two agree.
    fn conclude(
        &mut self,
        action: Action,
        record: &StorageRecord,
    ) -> Result<Vec<Command>, Failure> {
        let Some(before) = self.before.clone() else {
            return Err(self.reject("the action's own deciding read is missing"));
        };
        let Some(outcome) = self.outcome.clone() else {
            return Err(self.reject("the owners' answer to the transaction is missing"));
        };
        match &outcome {
            InventoryStorageOutcome::Committed => {
                // The owners answered a commit: the ledger must hold the value this
                // action asked for, at a version past the one it named.
                if record.value.as_deref() != Some(self.expected.as_str()) {
                    return Err(self.reject(&format!(
                        "committed, but the ledger reads {:?} where this action asked for {}",
                        record.value, self.expected
                    )));
                }
                let advanced = match (before.version, record.version) {
                    (None, Some(_)) => true,
                    (Some(read), Some(now)) => now > read,
                    _ => false,
                };
                if !advanced {
                    return Err(self.reject(&format!(
                        "committed, but the ledger version is {:?} where this action named {:?}",
                        record.version, before.version
                    )));
                }
            }
            InventoryStorageOutcome::Refused => {
                // The owners answered a refusal: both sides must be exactly the
                // ledger this action read before it asked.
                if record.value != before.value || record.version != before.version {
                    return Err(self.reject(&format!(
                        "refused, but the ledger moved from {:?}@{:?} to {:?}@{:?}",
                        before.value, before.version, record.value, record.version
                    )));
                }
            }
        }
        let count = match count_of(record) {
            Ok(count) => count,
            Err(detail) => return Err(self.reject(&detail)),
        };
        let outcome_name = match &outcome {
            InventoryStorageOutcome::Committed => "committed",
            InventoryStorageOutcome::Refused => "refused",
        };
        let Some(reporter) = self.reporter.clone() else {
            return Err(self.reject("no player is waiting for this action's marker"));
        };
        self.finish();
        Ok(vec![message_player(
            &reporter,
            format!("P3_TRADE {} {outcome_name} ledger={count}", action.name()),
        )])
    }

    /// Leave the action concluded: nothing outstanding until the next command.
    fn finish(&mut self) {
        self.action = None;
        self.step = Step::Idle;
        self.before = None;
        self.outcome = None;
        self.expected.clear();
    }

    /// The one failure this fixture reports: a line naming the action and the step
    /// an operator has to look at, and the plugin's own failure, so the deployment
    /// sees a callback that did not answer. A phase that ends here reports no
    /// marker.
    fn reject(&self, detail: &str) -> Failure {
        log(
            LogLevel::Error,
            &format!(
                "P3_TRADE_UNEXPECTED {} step {}: {detail}",
                self.action.map_or("-", Action::name),
                self.step.number()
            ),
        );
        Failure::Failed
    }
}

/// The two buttons this fixture's market shows: the one that buys and refunds at
/// slot 0, and the one that closes at slot 8.
///
/// A server-owned menu is a row of fixed slots, so the record carries the slot
/// index the plugin chose, the item the client sees there and the label it shows
/// with it. Both labels are the fixture's own wording, and both counts are one:
/// the menu is a row of buttons, not a second inventory.
fn market_slots() -> Vec<InventoryMenuSlot> {
    vec![
        InventoryMenuSlot {
            slot: BUY_SLOT,
            resource: APPLE.to_owned(),
            count: 1,
            label: Some(APPLE_LABEL.to_owned()),
        },
        InventoryMenuSlot {
            slot: CLOSE_SLOT,
            resource: BARRIER.to_owned(),
            count: 1,
            label: Some(CLOSE_LABEL.to_owned()),
        },
    ]
}

/// Which trade action one click means.
///
/// The four clicks a menu can report are two buttons: the plain and the
/// shifted primary click buy, the plain and the shifted secondary click refund.
/// A menu the server owns has no drag or drop, so every click it reports is one
/// of these four.
fn clicked_action(click: InventoryClick) -> Action {
    match click {
        InventoryClick::Primary | InventoryClick::ShiftPrimary => Action::Buy,
        InventoryClick::Secondary | InventoryClick::ShiftSecondary => Action::Refund,
    }
}

/// The purchased count one ledger record holds.
///
/// Absent means zero, the way this fixture's own key documents it. A value that is
/// not a decimal count is the fixture's own state being wrong, and is reported
/// rather than read as a number.
fn count_of(record: &StorageRecord) -> Result<i64, String> {
    match record.value.as_deref() {
        None => Ok(0),
        Some(text) => text
            .parse::<i64>()
            .map_err(|_| format!("the ledger holds {text:?}, which is not a decimal count")),
    }
}

/// Whether one event is the host answering a request this fixture made. Every
/// other event - a join, a chat line, another plugin's - is the world going on and
/// is none of this fixture's business.
fn is_answer(event: &Event) -> bool {
    matches!(
        event,
        Event::StorageGetAnswered(_) | Event::InventoryStorageTransactionAnswered(_)
    )
}

/// How one answer event is named in a diagnostic line, in the contract's own
/// vocabulary.
fn kind(event: &Event) -> &'static str {
    match event {
        Event::StorageGetAnswered(_) => "storage-get-answered",
        Event::InventoryStorageTransactionAnswered(_) => "inventory-storage-transaction-answered",
        _ => "event",
    }
}
