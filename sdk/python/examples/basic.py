from rp2350_hid_bridge import HidBridge, HidBridgeOptions


def main():
    # port=None 会通过 USB VID/PID 自动查找；也可以写 HidBridgeOptions(port="COM3")
    with HidBridge(HidBridgeOptions(port=None)) as hid:
        hid.ping()
        print("info:", hid.info().hex(" "))
        print("caps:", hid.caps().hex(" "))

        # 下面会产生真实 HID 输入，使用前确认当前焦点安全。
        hid.type_text("hello from python sdk")
        hid.key_tap("ENTER")
        hid.mouse_move(20, 0)
        hid.stop_all()


if __name__ == "__main__":
    main()
