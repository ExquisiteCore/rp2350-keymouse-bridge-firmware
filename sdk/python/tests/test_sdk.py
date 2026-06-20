import unittest

from rp2350_hid_bridge.keys import parse_combo
from rp2350_hid_bridge.protocol import CommandType, DecodeError, decode_frame, encode_frame
from rp2350_hid_bridge.script import parse_script


class ProtocolTests(unittest.TestCase):
    def test_frame_round_trip(self):
        frame = encode_frame(0x1234, CommandType.PING, b"")
        decoded = decode_frame(frame)

        self.assertEqual(decoded.version, 1)
        self.assertEqual(decoded.sequence, 0x1234)
        self.assertEqual(decoded.command_type, CommandType.PING)
        self.assertEqual(decoded.payload, b"")

    def test_bad_crc_is_rejected(self):
        frame = bytearray(encode_frame(7, CommandType.PING, b""))
        frame[-1] ^= 0x55

        with self.assertRaises(DecodeError):
            decode_frame(bytes(frame))

    def test_key_combo_parser(self):
        self.assertEqual(parse_combo("CTRL+C"), (0x01, 0x06))
        self.assertEqual(parse_combo("SHIFT+R"), (0x02, 0x15))
        self.assertEqual(parse_combo("ENTER"), (0x00, 0x28))
        self.assertEqual(parse_combo("F5"), (0x00, 0x3E))

    def test_script_parser(self):
        commands = parse_script(
            '''
type "abc"
key tap ENTER
mouse move 10 -5
wait 100
stop
'''
        )

        self.assertEqual(len(commands), 5)
        self.assertEqual(commands[0].kind, "type")
        self.assertEqual(commands[0].text, "abc")
        self.assertEqual(commands[1].kind, "key")
        self.assertEqual(commands[1].action, "tap")
        self.assertEqual(commands[2].kind, "mouse")
        self.assertEqual(commands[2].action, "move")
        self.assertEqual(commands[2].dx, 10)
        self.assertEqual(commands[2].dy, -5)
        self.assertEqual(commands[3].kind, "wait")
        self.assertEqual(commands[3].ms, 100)
        self.assertEqual(commands[4].kind, "stop")


if __name__ == "__main__":
    unittest.main()
