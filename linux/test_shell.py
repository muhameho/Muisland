"""Event logic regression tests; no real mouse input or provider sessions."""

import unittest
from types import SimpleNamespace
from app import Shell


class OutsideClickTests(unittest.TestCase):
    def setUp(self):
        self.shell = Shell.__new__(Shell)
        self.shell.open = True
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


if __name__ == "__main__":
    unittest.main()
