"""harness-sync manager: browse sources and mirrors, delete a group.

Keys:
  up/down        move
  ctrl+r         reload from disk
  ctrl+d         arm delete of the selected group; press ctrl+d again within
                 CONFIRM_SECONDS to delete. Any other key cancels.
  q              quit

A delete removes the source and every mirror of it (see hs_manage). Nothing
is deleted from the list view itself; the confirmation is the only path.
"""
from __future__ import annotations

import time
from datetime import datetime

from textual import on
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import DataTable, Footer, Header, Input, OptionList, Static
from textual.widgets.option_list import Option

import harness_sync as hs
import hs_manage as hm

CONFIRM_SECONDS = 4.0


ACTIONS = [
    ("rename", "Rename", "change the name this session is listed under"),
    ("delete", "Delete source and all mirrors", "needs ctrl+d twice after choosing"),
    ("reload", "Reload list", "re-read state and session stores"),
    ("close", "Close", "back to the list"),
]

DUPE_HINT = "suggested duplicate"


class ActionPanel(ModalScreen[str | None]):
    """Keyboard-only action list for one conversation.

    Up/Down move, Enter runs the highlighted action, Escape closes. The
    highlight starts on Close, so a stray Enter cannot start a delete."""

    BINDINGS = [
        Binding("escape", "close", "Close", show=True),
        Binding("q", "close", "Close", show=False),
    ]
    DEFAULT_CSS = """
    ActionPanel { align: center middle; background: $background 60%; }
    #panel { width: 72; height: auto; max-height: 90%; border: heavy $accent; padding: 0 1;
             background: $background; color: $text; }
    #panel-title { text-style: bold; color: $accent; }
    #panel-meta { color: $text-muted; margin-bottom: 1; }
    #actions { height: auto; max-height: 10; background: $background; border: none; padding: 0; }
    #actions > .option-list--option-highlighted { background: $accent; color: $text; text-style: bold; }
    #panel-hint { color: $text-muted; margin-top: 1; }
    #panel-dupe { color: $warning; margin-bottom: 1; }
    """

    def __init__(self, group: hm.Group, dupe: hm.DuplicateSet | None = None) -> None:
        super().__init__()
        self.group = group
        self.dupe = dupe

    def compose(self) -> ComposeResult:
        g = self.group
        mirrors = ", ".join(m.target for m in g.mirrors) or "none"
        with Vertical(id="panel"):
            yield Static(g.title, id="panel-title")
            yield Static(f"{g.fmt} · mirrors: {mirrors}\n{g.key}", id="panel-meta")
            if self.dupe is not None:
                keep = self.dupe.keep
                yield Static(f"suggested duplicate ({self.dupe.reason}). Keeping '{keep.title}' "
                             f"({keep.fmt}, {keep.key.split('/')[-1][:40]}). Delete this copy?",
                             id="panel-dupe")
            yield OptionList(*[Option(f"{label}  [dim]{hint}[/dim]", id=key) for key, label, hint in ACTIONS],
                             id="actions")
            yield Static("up/down move · enter run · esc close", id="panel-hint")

    def on_mount(self) -> None:
        options = self.query_one("#actions", OptionList)
        options.highlighted = len(ACTIONS) - 1  # Close
        options.focus()

    @on(OptionList.OptionSelected, "#actions")
    def _run(self, event: OptionList.OptionSelected) -> None:
        self.dismiss(event.option.id)

    def action_close(self) -> None:
        self.dismiss(None)


class ThemePicker(ModalScreen[str | None]):
    """Lists every theme and shows the one under the cursor. Enter applies it,
    Escape keeps the theme that was active when the picker opened."""

    BINDINGS = [Binding("escape", "cancel", "Cancel")]
    DEFAULT_CSS = """
    ThemePicker { align: center middle; }
    #theme-panel { width: 50; height: 80%; border: round $accent; padding: 1 2; background: $background; }
    #theme-status { height: 3; margin-bottom: 1; padding: 0 1; background: $accent; color: $text; text-style: bold; }
    """

    def __init__(self, current: str) -> None:
        super().__init__()
        self.original = current
        self.names = sorted(self.app.available_themes)

    def compose(self) -> ComposeResult:
        with Vertical(id="theme-panel"):
            yield Static("", id="theme-status")
            yield OptionList(*self.names, id="themes")

    def on_mount(self) -> None:
        options = self.query_one(OptionList)
        if self.original in self.names:
            options.highlighted = self.names.index(self.original)
        options.focus()
        self._show(self.original)

    @on(OptionList.OptionHighlighted)
    def _highlight(self, event: OptionList.OptionHighlighted) -> None:
        name = self.names[event.option_index]
        self.app.theme = name  # live preview while browsing
        self._show(name)

    @on(OptionList.OptionSelected)
    def _select(self, event: OptionList.OptionSelected) -> None:
        self.dismiss(self.names[event.option_index])

    def action_cancel(self) -> None:
        self.app.theme = self.original
        self.dismiss(None)

    def _show(self, name: str) -> None:
        marker = " (active)" if name == self.app.theme else ""
        self.query_one("#theme-status", Static).update(f"highlighted: {name}{marker}")


class RenameScreen(ModalScreen[str | None]):
    """Single text field. Enter saves (validated by the data layer), Escape cancels."""

    BINDINGS = [Binding("escape", "cancel", "Cancel")]
    DEFAULT_CSS = """
    RenameScreen { align: center middle; background: $background 60%; }
    #rename-panel { width: 72; height: auto; border: heavy $accent; padding: 0 1; background: $background; }
    #rename-title { text-style: bold; color: $accent; }
    #rename-hint { color: $text-muted; }
    #rename-error { color: $error; height: auto; }
    """

    def __init__(self, group: hm.Group) -> None:
        super().__init__()
        self.group = group

    def compose(self) -> ComposeResult:
        with Vertical(id="rename-panel"):
            yield Static(f"Rename: {self.group.title}", id="rename-title")
            yield Input(value=self.group.title, placeholder="new name", id="rename-input", max_length=200)
            yield Static("enter save · esc cancel · max 80 characters", id="rename-hint")
            yield Static("", id="rename-error")

    def on_mount(self) -> None:
        self.query_one(Input).focus()

    @on(Input.Submitted, "#rename-input")
    def _submit(self, event: Input.Submitted) -> None:
        try:
            hm.validate_title(event.value)
        except hm.DeleteRefused as exc:
            self.query_one("#rename-error", Static).update(str(exc))
            return
        self.dismiss(event.value)

    def action_cancel(self) -> None:
        self.dismiss(None)


class ManageApp(App):
    TITLE = "harness-sync"
    CSS = """
    #status { height: 3; padding: 0 1; }
    DataTable { height: 1fr; }
    """
    BINDINGS = [
        Binding("ctrl+r", "reload", "Reload"),
        Binding("ctrl+d", "delete", "Delete (press twice)"),
        Binding("ctrl+t", "pick_theme", "Theme"),
        Binding("d", "next_duplicate", "Next duplicate"),
        Binding("t", "next_trivial", "Next trivial"),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self.groups: list[hm.Group] = []
        self._armed: tuple[str, float] | None = None  # (group key, armed at)

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static("", id="status")
        table = DataTable(cursor_type="row", zebra_stripes=True)
        table.add_columns("source", "title", "mirrors", "updated", "lock")
        yield table
        yield Footer()

    def on_mount(self) -> None:
        self.action_reload()

    # ------------------------------------------------------------ data

    def _load(self) -> None:
        state = hs.State()
        self.groups = hm.list_groups(state)
        self.dupes = hm.find_duplicates(self.groups)
        self._dupe_of = {r.key: d for d in self.dupes for r in d.remove}
        self.trivial = hm.find_trivial(self.groups)
        self._trivial_keys = {t.group.key for t in self.trivial}
        table = self.query_one(DataTable)
        table.clear()
        for g in self.groups:
            mirrors = ", ".join(f"{m.target}{' (frozen)' if m.diverged else ''}" for m in g.mirrors) or "-"
            updated = datetime.fromtimestamp(g.updated).strftime("%Y-%m-%d %H:%M") if g.updated else "-"
            lock = self._lock_text(g)
            name = g.key.split("/")[-1] if g.fmt != "opencode" else g.key
            badge = " (orphan)" if g.orphan else ""
            if g.key in self._dupe_of:
                badge += " [dupe]"
            if g.key in self._trivial_keys:
                badge += " [trivial]"
            table.add_row(g.fmt + badge, g.title, mirrors, updated, lock, key=g.key)
        n = len(self.dupes)
        t = len(self.trivial)
        extra = ""
        if n:
            extra += f", {n} duplicate set(s)"
        if t:
            extra += f", {t} trivial session(s)"
        if extra:
            extra += "; d = next duplicate, t = next trivial"
        self._status(f"{len(self.groups)} conversation(s){extra}. ctrl+d twice on a row to delete it.")

    @staticmethod
    def _lock_text(g: hm.Group) -> str:
        # hslock stores locks under <lock dir>/<id>.lock, with the id reduced to
        # [A-Za-z0-9_-] (see lock/src/main.rs safe_id). Mirror that here.
        safe = "".join(c for c in g.lock_id if c.isascii() and (c.isalnum() or c in "-_"))
        p = hm.lock_dir() / f"{safe}.lock"
        if not p.exists():
            return "free"
        try:
            return "held: " + p.read_text(encoding="utf-8").split(" ", 1)[0]
        except OSError:
            return "?"

    def _status(self, msg: str) -> None:
        self.query_one("#status", Static).update(msg)

    def _selected(self) -> hm.Group | None:
        table = self.query_one(DataTable)
        if not self.groups or table.cursor_row is None:
            return None
        row = table.cursor_row
        return self.groups[row] if 0 <= row < len(self.groups) else None

    # ---------------------------------------------------------- actions

    def action_reload(self) -> None:
        self._armed = None
        self._load()

    def action_next_trivial(self) -> None:
        """Move the highlight to the next row flagged as a trivial session."""
        table = self.query_one(DataTable)
        flagged = [i for i, g in enumerate(self.groups) if g.key in self._trivial_keys]
        if not flagged:
            self._status("no trivial sessions to suggest")
            return
        current = table.cursor_row or 0
        nxt = next((i for i in flagged if i > current), flagged[0])
        table.move_cursor(row=nxt)
        t = next(x for x in self.trivial if x.group.key == self.groups[nxt].key)
        self._status(f"trivial: {t.user_turns} prompt(s), no tool use, idle {t.idle_days:.0f}d. "
                     f"Enter to review, ctrl+d twice to delete.")

    def action_next_duplicate(self) -> None:
        """Move the highlight to the next row flagged as a suggested duplicate."""
        table = self.query_one(DataTable)
        flagged = [i for i, g in enumerate(self.groups) if g.key in self._dupe_of]
        if not flagged:
            self._status("no suggested duplicates")
            return
        current = table.cursor_row or 0
        nxt = next((i for i in flagged if i > current), flagged[0])
        table.move_cursor(row=nxt)
        g = self.groups[nxt]
        d = self._dupe_of[g.key]
        self._status(f"duplicate ({d.reason}): keeping '{d.keep.title}' ({d.keep.fmt}). "
                     f"Enter to review, ctrl+d twice to delete this copy.")

    def action_delete(self) -> None:
        g = self._selected()
        if g is None:
            self._status("nothing selected")
            return
        now = time.monotonic()
        if self._armed and self._armed[0] == g.key and now - self._armed[1] <= CONFIRM_SECONDS:
            self._armed = None
            self._delete(g)
            return
        self._armed = (g.key, now)
        self._status(f"Press ctrl+d again to DELETE '{g.title}' ({len(g.mirrors)} mirror(s) + source). "
                     f"Any other key cancels.")

    def _delete(self, g: hm.Group) -> None:
        try:
            done = hm.delete_group(g)
        except hm.DeleteRefused as exc:
            self._status(f"refused: {exc}")
            return
        except Exception as exc:  # keep the app alive; report what failed
            self._status(f"delete failed: {exc}. Run reload to see what is left.")
            return
        self._status(f"deleted {len(done)} item(s) for '{g.title}'")
        self._load()

    def on_key(self, event) -> None:
        # Any key other than ctrl+d cancels a pending delete.
        if event.key not in ("ctrl+d",) and self._armed is not None:
            self._armed = None
            self._status("delete cancelled")

    @on(DataTable.RowSelected)
    def _on_select(self) -> None:
        self._armed = None
        g = self._selected()
        if g is not None:
            self.push_screen(ActionPanel(g, self._dupe_of.get(g.key)), self._on_action)

    def _on_action(self, result: str | None) -> None:
        g = self._selected()
        if result == "rename" and g is not None:
            self.push_screen(RenameScreen(g), self._on_rename)
        elif result == "delete" and g is not None:
            self._armed = (g.key, time.monotonic())
            self._status(f"Press ctrl+d again to DELETE '{g.title}'. Any other key cancels.")
        elif result == "reload":
            self.action_reload()

    def _on_rename(self, new_title: str | None) -> None:
        if not new_title:
            self._status("rename cancelled")
            return
        g = self._selected()
        if g is None:
            self._status("nothing selected")
            return
        try:
            saved = hm.rename_group(g, new_title)
        except hm.DeleteRefused as exc:
            self._status(f"rename refused: {exc}")
            return
        except Exception as exc:
            self._status(f"rename failed: {exc}")
            return
        self._load()  # reload first: _load sets the status line itself
        self._status(f"renamed to '{saved}'. Mirrors update on the next sync pass.")

    # ----------------------------------------------------------- theme

    def action_pick_theme(self) -> None:
        self.push_screen(ThemePicker(self.theme), self._on_theme)

    # Textual's own "Theme" command (Ctrl+P palette) only applies a theme on
    # Enter, so it never previews while browsing. Route it to our picker.
    def action_change_theme(self) -> None:
        self.action_pick_theme()

    def _on_theme(self, name: str | None) -> None:
        if name:
            self.theme = name
        self._status(f"theme: {self.theme}")


if __name__ == "__main__":
    ManageApp().run()
