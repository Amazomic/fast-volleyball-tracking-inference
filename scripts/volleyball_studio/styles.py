PRESETS = {
    "classic": {"effects": "off", "accents": False, "grade": False, "titles": False, "speed_ramp": False},
    "dynamic": {"effects": "subtle", "accents": True, "grade": True, "titles": True, "speed_ramp": True},
    "cinematic": {"effects": "cinematic", "accents": True, "grade": True, "titles": True, "speed_ramp": True},
}


def apply_style(args):
    preset = PRESETS[args.style]
    if args.effects is None:
        args.effects = preset["effects"]
    args.accents = args.accents or preset["accents"]
    args.grade = preset["grade"] and args.effects != "off"
    args.titles = preset["titles"] and args.effects != "off"
    args.speed_ramp = preset["speed_ramp"] and args.effects != "off"
    return args
