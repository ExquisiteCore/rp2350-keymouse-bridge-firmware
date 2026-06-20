from rp2350_hid_bridge import find_port, list_ports


def _hex_or_dash(value):
    return f"{value:04X}" if value is not None else "----"


def main():
    print("device\tvid:pid\tproduct\tserial")
    for port in list_ports():
        vid = _hex_or_dash(getattr(port, "vid", None))
        pid = _hex_or_dash(getattr(port, "pid", None))
        product = getattr(port, "product", "") or ""
        serial = getattr(port, "serial_number", "") or ""
        print(f"{port.device}\t{vid}:{pid}\t{product}\t{serial}")

    detected = find_port()
    print(f"\nRP2350 bridge: {detected or 'not found'}")


if __name__ == "__main__":
    main()
