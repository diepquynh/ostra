import unittest

from shop.money import format_money


class FormatMoneyTest(unittest.TestCase):
    def test_under_a_thousand_dollars_has_no_separator(self):
        self.assertEqual(format_money(99975), "$999.75")

    def test_thousands_are_separated(self):
        self.assertEqual(format_money(123325), "$1,233.25")

    def test_millions_are_separated(self):
        self.assertEqual(format_money(100000000), "$1,000,000.00")

    def test_negative_amount_keeps_the_minus_before_the_dollar_sign(self):
        self.assertEqual(format_money(-123456), "-$1,234.56")

    def test_small_negative_amount(self):
        self.assertEqual(format_money(-5), "-$0.05")
