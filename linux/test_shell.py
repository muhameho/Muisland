"""Event logic regression tests; no real mouse input or provider sessions."""

import unittest
from unittest.mock import patch
from types import SimpleNamespace
from app import Shell


class OutsideClickTests(unittest.TestCase):
    def setUp(self):
        self.shell = Shell.__new__(Shell)
        self.shell.open = True
        self.shell.down = None
        self.shell.wayland = False
        self.shell.outside_buttons = 0
        self.shell.windows = {
            "menu": SimpleNamespace(
                get_position=lambda: (100, 100), get_size=lambda: (420, 628)
            )
        }
        self.shell.close_menu = lambda: setattr(self.shell, "open", False)

    def event(self, x, y, buttons):
        self.shell.pointer_buttons = lambda: (x, y, buttons)
        self.shell.watch_outside_click()

    def test_leaving_pointer_without_click_keeps_menu_open(self):
        self.event(120, 120, 0)
        self.event(800, 800, 0)
        self.assertTrue(self.shell.open)

    def test_only_a_new_outside_button_press_collapses(self):
        self.event(120, 120, 256)
        self.assertTrue(self.shell.open)
        self.event(800, 800, 256)
        self.assertTrue(self.shell.open)
        self.event(800, 800, 0)
        self.event(800, 800, 256)
        self.assertFalse(self.shell.open)

    def test_fast_drag_uses_absolute_origin_and_finishes_outside_widget(self):
        s = self.shell
        s.open = False
        s.down = ("float", 110, 110, 0, 10, 10)
        s.drag_anchor = (100, 100)
        s.dragging = False
        s.layer = False
        s.layer_motion = None
        s.move_float = lambda: None
        s.save_position = lambda: None
        s.pointer_buttons = lambda: (900, 700, 256)
        s.watch_outside_click()
        self.assertEqual(s.floating, (890, 690))
        self.assertTrue(s.dragging)
        s.pointer_buttons = lambda: (1200, 900, 0)
        s.watch_outside_click()
        self.assertEqual(s.floating, (1190, 890))
        self.assertIsNone(s.down)
        self.assertFalse(s.dragging)

    def test_stale_drag_completion_cannot_stop_a_new_drag(self):
        s = self.shell
        s.drag_sequence = 2
        s.dragging = True
        self.assertFalse(s.drag_finished(1))
        self.assertTrue(s.dragging)

    def test_queued_wayland_motion_is_applied_once_per_frame(self):
        s = self.shell
        s.down = ("float", 0, 0, 0, 10, 10)
        s.floating = (100, 100)
        s.layer_motion = (60, 40)
        s.dragging = False
        s.move_float = lambda: None
        s.apply_layer_motion()
        self.assertEqual(s.floating, (150, 130))
        s.apply_layer_motion()
        self.assertEqual(s.floating, (150, 130))

    def test_quit_waits_for_profile_transaction(self):
        self.shell.service = SimpleNamespace(busy=True)
        self.shell.quit_when_idle = False
        with patch("app.Gtk.main_quit") as quit:
            self.shell.request_quit()
            self.assertTrue(self.shell.quit_when_idle)
            quit.assert_not_called()
            self.shell.service.busy = False
            self.shell.request_quit()
            quit.assert_called_once()


if __name__ == "__main__":
    unittest.main()
