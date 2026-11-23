import unittest

from bank import labels


class Labels(unittest.TestCase):
    def test_small(self):
        self.assertEqual(labels.label(5), "small")
