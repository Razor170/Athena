//! The text of the Welcome tab's guide.
//!
//! Kept apart from the drawing code so it is easy to edit when a module
//! changes. A few markers format the text (see `mod.rs`):
//!
//! - `**Mining**` shows "Mining" in bold.
//! - `[End session]` shows "End session" like a button name.
//! - `{green}`, `{purple}`, `{yellow}` and `{red}` show a small coloured
//!   square, the colours of the mining hold bars.

/// One collapsible section, one per module.
pub struct Section {
    pub title: &'static str,
    /// Shown dimmed next to the title.
    pub subtitle: &'static str,
    pub paragraphs: &'static [&'static str],
    /// Shown dimmed after the paragraphs. Empty for none.
    pub note: &'static str,
}

pub const INTRO: &str = "Athena is a companion for EVE Online. It reads EVE's log files and \
the ESI web API. It never reads from or sends anything to the game client.";

pub const GETTING_STARTED: &[&str] = &[
    "**Settings** › EVE logs: check that the log folder is right. The default is \
     Documents\\EVE\\logs.",
    "**Characters**: click [+ Add character] and log in with EVE. Finance, Planetary, \
     Contracts and Skills need this. Mining and Intel work without it.",
    "**Intel** › Settings: tick the intel channels you want watched.",
    "**Mining** needs no setup. Start mining and a card shows up for each character.",
];

pub const TABS_AND_WINDOWS: &[&str] = &[
    "The buttons in the top bar open and close modules. Bright means open. Settings sits \
     on the right.",
    "Drag a tab by its title to dock it next to another tab, or pull it out into its own \
     window.",
    "Athena remembers the layout, the window position and where closed tabs were.",
];

pub const SECTIONS: &[Section] = &[
    Section {
        title: "Mining",
        subtitle: "ore holds, alerts and history from your game logs",
        paragraphs: &[
            "**Live** shows one card per character. Bars: {green} ore, {purple} compressed \
             ore, {yellow} past your warning level, {red} full. The purple % is the \
             compressed share of the hold.",
            "A red dashed border means the miner stopped (also when the asteroid is gone). \
             A purple dashed border means a crystal broke.",
            "Click [Emptied] after you unload a hold, or [Empty all] in the header.",
            "**Belt scan:** in EVE, select the Mining Survey Scanner results and press \
             Ctrl+C. Athena picks it up from the clipboard and shows m³ left, rocks left \
             and time to clear. Scan again later to update the rate. The bar is full at \
             the first scan; its striped part is the estimate since the last scan.",
            "ISK is the reprocessed value at Jita prices, refreshed every hour.",
            "[End session] saves the session for all characters to **History**, with daily \
             and weekly totals.",
        ],
        note: "Settings: hold size and warning level (for all or per character), columns, \
               which alerts beep, character order, hold bars overlay.",
    },
    Section {
        title: "Intel",
        subtitle: "hostile reports from your intel channels on a map",
        paragraphs: &[
            "The map shows the region of the character you follow. Reported systems light \
             up red and fade to grey over 10 minutes.",
            "Athena beeps and flashes the taskbar when a report is 5 jumps away or less, \
             and louder at 2 or less. [Mute 5 min] silences it for a while, \
             [Silence Alarms] also stops the popup, until you click [Enable Alarms] \
             (or restart Athena).",
            "Untick a character under [Pilots] to hide it from the map and from alerts.",
            "Right-click a system for **Add tag…**, **Open on Dotlan** and **Mark clear**. \
             Pilot names in the intel list open zKillboard.",
        ],
        note: "Settings: intel channels, alert range, beep or voice, popup, tags (import and \
               export), kills tint.",
    },
    Section {
        title: "Overlays",
        subtitle: "small windows on top of EVE",
        paragraphs: &[
            "**Hold bars:** Mining › Settings › **Show hold bars overlay**. Use \
             [Move / resize overlay] to place it. Clicks go through it to EVE.",
            "**Intel popup:** Intel › Settings › **Show a popup on intel alerts**. Use \
             [Show popup to move it] to place it. It never takes focus from EVE.",
        ],
        note: "Overlays only show over EVE in Window or Fixed Window mode, not Fullscreen.",
    },
    Section {
        title: "Characters",
        subtitle: "EVE logins for the ESI modules",
        paragraphs: &[
            "[+ Add character] opens the EVE login in your browser. Athena asks for every \
             permission once, so you never need to log in again for a new module.",
            "Your login is kept in Windows Credential Manager, never in Athena's files.",
            "Drag characters into accounts (3 slots each) and give them tags to filter \
             other modules.",
            "If a character shows **Login expired** or **New scopes**, click \
             [Log in again].",
        ],
        note: "",
    },
    Section {
        title: "Skills",
        subtitle: "what every character trains and when the queue ends",
        paragraphs: &[
            "**Overview** shows a card per logged-in character: the skill in training with \
             its time left, and when the whole queue ends. Red means not training (empty or \
             paused queue, or no data), yellow means the queue ends within 24 h. Click a \
             card to open the character.",
            "[Queue end], [Name] and [SP] sort the cards, tag chips show only characters \
             with that tag. The number on the Skills button counts the characters that \
             aren't training. Right-click a card to hide a character (no warning, not \
             counted); [Show hidden] or the list in Settings brings it back.",
            "**Character** looks like EVE's skill window: the skill groups on top, the \
             picked group's skills below and the training queue on the right. White squares \
             are trained levels, blue ones are queued, grey ones are trained but limited by \
             an Alpha clone. The search box looks through all groups, **Show not injected** \
             also lists skills you don't have.",
            "Under the queue: the total training time, when it ends, and how many large and \
             small skill injectors would finish it right now (unallocated SP counts first), \
             and what they cost at the lowest Jita sell price.",
            "**Plans** holds skill plans for all characters. Click level squares on the left \
             or type a name to add skills (prerequisites come along), drag to reorder, and \
             [Save]. Import reads a pasted list, Copy for EVE puts the plan on the clipboard \
             for EVE's skill plan import. Check times the plan for a character with each \
             implant set and their clones, and shows the best remap and the time it saves. \
             Accelerator adds a cerebral accelerator used back to back for the whole plan: \
             how many you need and what's left on the last one (Biology and BY implants make \
             them last longer). If one is running already, pick None.",
        ],
        note: "Settings: the \"ends soon\" hours, the badge, and how often ESI is asked \
               (every 15 minutes by default; [Refresh] asks now).",
    },
    Section {
        title: "Finance",
        subtitle: "wallets, income and LP",
        paragraphs: &[
            "**Wallets**: click cards to pick which characters count. **All characters** picks \
             every one.",
            "**Overview**: income and expenses by category. Click a slice to see what's in \
             it. The chart below switches between Balance and Net. Scroll to zoom, drag to \
             move, double-click to reset.",
            "**Transactions**: every entry, with filters and search. **Loyalty Points**: LP \
             per character and the nearest LP store.",
        ],
        note: "Money moved between your picked characters is left out of Overview. ESI only \
               keeps 30 days, so Athena saves entries and your history grows over time.",
    },
    Section {
        title: "Planetary",
        subtitle: "which colonies need you",
        paragraphs: &[
            "**Planets** shows a card per logged-in character with one row per planet, red \
             first. Red needs attention (extractor stopped, pad or storage full, factory \
             inputs gone). Yellow is soon (extractor ends within 24 h, inputs under 12 h, \
             a pad at 75%). Green is fine.",
            "Each row has one bar per launchpad. ESI only updates a colony when you open it \
             in EVE, so each row says when it was viewed. The striped part of a bar is the \
             estimated fill since then.",
            "Click a row for extractors, factories, inputs and what's on each pad.",
            "[Needs attention] hides the planets that are fine. Tag chips show only \
             characters with that tag. [Refresh] asks ESI now; otherwise Athena asks every \
             10 minutes.",
        ],
        note: "Settings: the hours, pad levels and days that count as soon, full or old, and \
               the fill estimate on or off.",
    },
    Section {
        title: "Contracts",
        subtitle: "your contracts and what changed",
        paragraphs: &[
            "The list on the left shows every contract of your characters. [Active], \
             [Finished] and [All], the type buttons, tag chips and the search box filter it. \
             Click a row for the route, bids, who accepted it and the items.",
            "The changelog on the right keeps every change: accepted, sold, delivered, new \
             bid, expired, due soon and more. Drag the bar between them to resize.",
            "Changes from while Athena was closed come first, marked **while closed**, with a \
             dot on their row. Click one (or [Mark all seen]) and it moves into its day. The \
             number on the Contracts button counts what you haven't seen.",
        ],
        note: "Settings: which changes alert (count and taskbar flash) and beep, the \
               \"soon\" hours, and how long finished contracts stay in the list. Athena only \
               reads contracts; accept them in EVE.",
    },
    Section {
        title: "Fittings",
        subtitle: "compare a fit with the one it should be",
        paragraphs: &[
            "**Compare**: put the fit you have in **Is** and the fit you want in **Should**. \
             In EVE's fitting window use \"Copy to clipboard\", then click [Paste] on the \
             column. [Text] shows the fit as text to edit, [List] as a list with duplicate \
             lines added up.",
            "[Open ship…] loads one of your ships from ESI assets: pick a station on the \
             left, then the ship on the right. ESI keeps assets for up to an hour, so the \
             window shows how old each character's data is and when [Refresh] has newer \
             data.",
            "[Save…] keeps a column's fit under a name, [Saved…] loads one again. The \
             list can be searched by name, filtered by ship type and sorted by when a fit \
             was last changed.",
            "**Diff** shows what is missing in green (+) and what is too much in red (−), by \
             rigs, high, med and low slots, drones and fighters, and cargo. A module in cargo \
             only counts against cargo.",
            "[Copy missing] copies the green items for EVE's Multibuy. [Swap] exchanges the \
             two fits. **Show matching** also lists what both fits have.",
        ],
        note: "Both fits are kept when Athena restarts.",
    },
    Section {
        title: "Settings and updates",
        subtitle: "",
        paragraphs: &[
            "**Settings** has the EVE log folder, how often logs are checked, where Athena \
             keeps its data and the update options.",
            "When a new version is out, a strip appears under the top bar. Click \
             [Update now], then [Restart now] or [On next start]. Settings, layout and \
             history are kept.",
        ],
        note: "Updates need a read-only GitHub token, pasted once in Settings › Updates.",
    },
];
