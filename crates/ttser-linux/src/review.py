"""Embedded GTK editor; transcripts travel only through private process pipes."""
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gdk, GdkX11, Gtk


class ReviewWindow(Gtk.Window):
    def __init__(self, text, parent):
        super().__init__(title="ttser — popraw transkrypcję")
        self.accepted = None
        self.pending_enter = False
        self.set_default_size(620, 240)
        self.set_type_hint(Gdk.WindowTypeHint.DIALOG)
        self.set_modal(True)
        self.set_position(Gtk.WindowPosition.CENTER)
        self.set_border_width(14)
        self.connect("delete-event", self.cancel)
        self.connect("key-press-event", self.key_press)
        self.connect("key-release-event", self.key_release)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add(box)
        self.editor = Gtk.TextView()
        self.editor.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.editor.set_top_margin(8)
        self.editor.set_bottom_margin(8)
        self.editor.set_left_margin(8)
        self.editor.set_right_margin(8)
        self.buffer = self.editor.get_buffer()
        self.buffer.set_text(text)
        self.buffer.place_cursor(self.buffer.get_end_iter())
        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
        scroll.add(self.editor)
        box.pack_start(scroll, True, True, 0)
        footer = Gtk.Box(spacing=12)
        footer.pack_start(Gtk.Label(label="Enter: wklej  ·  Shift+Enter: nowa linia  ·  Esc: anuluj"), True, True, 0)
        submit = Gtk.Button(label="Wklej")
        submit.connect("clicked", self.submit)
        footer.pack_end(submit, False, False, 0)
        box.pack_end(footer, False, False, 0)
        self.realize()
        foreign = GdkX11.X11Window.foreign_new_for_display(self.get_display(), parent)
        if foreign is not None:
            self.get_window().set_transient_for(foreign)
        # Selecting a correction must not overwrite the user's PRIMARY selection.
        self.editor.connect("realize", lambda view: self.buffer.remove_selection_clipboard(Gtk.Clipboard.get(Gdk.SELECTION_PRIMARY)))

    def cancel(self, *_):
        Gtk.main_quit()
        return True

    def submit(self, *_):
        self.accepted = self.buffer.get_text(*self.buffer.get_bounds(), True)
        Gtk.main_quit()

    def key_press(self, _, event):
        if event.keyval == Gdk.KEY_Escape:
            return self.cancel()
        if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter):
            if event.state & Gdk.ModifierType.SHIFT_MASK:
                return False
            # Keep the dialog focused until release, including during key repeat.
            self.pending_enter = True
            return True
        return False

    def key_release(self, _, event):
        if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter) and self.pending_enter:
            self.pending_enter = False
            self.submit()
            return True
        return False


def main():
    window = ReviewWindow(sys.stdin.buffer.read().decode("utf-8"), int(sys.argv[1]))
    window.show_all()
    window.editor.grab_focus()
    window.present()
    Gtk.main()
    window.destroy()
    if window.accepted is None:
        return 2
    sys.stdout.buffer.write(window.accepted.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
