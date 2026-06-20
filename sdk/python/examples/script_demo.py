import argparse

from rp2350_hid_bridge import HidBridge, HidBridgeOptions, parse_script


SCRIPT = '''
type "hello from ExquisiteCore"
key tap ENTER
mouse move 20 0
wait 100
stop
'''


def main():
    parser = argparse.ArgumentParser(description="Parse or run an RP2350 HID bridge script.")
    parser.add_argument("--run", action="store_true", help="send the script to the device")
    parser.add_argument("--port", default=None, help="serial port, for example COM3")
    args = parser.parse_args()

    if not args.run:
        for command in parse_script(SCRIPT):
            print(command)
        print("\nUse --run --port COMx to send real HID input.")
        return

    with HidBridge(HidBridgeOptions(port=args.port)) as hid:
        hid.run_script(SCRIPT)


if __name__ == "__main__":
    main()
