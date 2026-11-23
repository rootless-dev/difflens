import unittest

from bank.account import apply_fee, can_withdraw, double_steps


class CanWithdraw(unittest.TestCase):
    def test_enough(self):
        self.assertTrue(can_withdraw(100, 30))

    def test_not_enough(self):
        self.assertFalse(can_withdraw(10, 30))


class ApplyFee(unittest.TestCase):
    def test_fee(self):
        self.assertEqual(apply_fee(100, 0.5), 150)


class Steps(unittest.TestCase):
    def test_three_rounds(self):
        self.assertEqual(double_steps(3), 6)
