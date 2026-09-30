import unittest

from shop.money import format_money


class FormatMoneyTest(unittest.TestCase):
    def test_dollars_and_cents(self):
        self.assertEqual(format_money(1234), "$12.34")

    def test_pads_cents_below_ten(self):
        self.assertEqual(format_money(1205), "$12.05")

    def test_zero(self):
        self.assertEqual(format_money(0), "$0.00")

    def test_less_than_a_dollar(self):
        self.assertEqual(format_money(5), "$0.05")

    def test_negative_amount_puts_the_minus_before_the_dollar_sign(self):
        self.assertEqual(format_money(-5), "-$0.05")

    def test_negative_dollars(self):
        self.assertEqual(format_money(-123456), "-$1234.56")
