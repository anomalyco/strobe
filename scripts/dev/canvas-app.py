#!/usr/bin/env python3
# A custom-drawn GTK 3 window for strobe's image anchors and pixel waits (dev fixture, not shipped): one
# DrawingArea painted with cairo, no accessible children, so only its pixels say what it shows. The same
# tiles as canvas-app.cs: a click on PLAY or Space counts (one bar each), a click on STOP or R resets, X
# shows or hides a square. `canvas-app.py --anchors <dir>` writes each tile as a PNG (PLAY at 1.5x and 2x
# too) and exits.
import math
import os
import sys

import cairo
import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gtk

PLAY, STOP, ITEM_A, ITEM_B, SQUARE = (40, 40, 160, 60), (240, 40, 160, 60), (40, 160, 120, 50), (240, 160, 120, 50), (440, 160, 100, 100)


def rgb(c, r, g, b):
    c.set_source_rgb(r / 255, g / 255, b / 255)


def text(c, s, x, y):
    c.select_font_face("Sans", cairo.FONT_SLANT_NORMAL, cairo.FONT_WEIGHT_BOLD)
    c.set_font_size(14)
    c.move_to(x, y + 14)
    c.show_text(s)


def play(c):
    rgb(c, 40, 160, 80)
    c.rectangle(0, 0, 160, 60)
    c.fill()
    c.set_source_rgb(1, 1, 1)
    c.set_line_width(3)
    c.rectangle(6, 6, 148, 48)
    c.stroke()
    rgb(c, 16, 48, 16)
    c.move_to(24, 16)
    c.line_to(24, 44)
    c.line_to(48, 30)
    c.fill()
    c.set_source_rgb(1, 1, 1)
    text(c, "PLAY", 70, 20)


def stop(c):
    rgb(c, 190, 50, 50)
    c.rectangle(0, 0, 160, 60)
    c.fill()
    rgb(c, 48, 16, 16)
    c.rectangle(22, 16, 28, 28)
    c.fill()
    c.set_source_rgb(1, 1, 1)
    text(c, "STOP", 70, 20)


def item(c):
    rgb(c, 40, 80, 190)
    c.rectangle(0, 0, 120, 50)
    c.fill()
    rgb(c, 240, 210, 40)
    c.arc(60, 25, 15, 0, 2 * math.pi)
    c.fill()


def square(c):
    rgb(c, 240, 140, 30)
    c.rectangle(0, 0, 100, 100)
    c.fill()
    c.set_source_rgb(0, 0, 0)
    for x, y in ((50, 15), (85, 50), (50, 85), (15, 50)):
        c.line_to(x, y)
    c.fill()


def missing(c):
    rgb(c, 150, 40, 170)
    c.rectangle(0, 0, 80, 40)
    c.fill()
    c.set_source_rgb(1, 1, 1)
    c.arc(40, 20, 15, 0, 2 * math.pi)
    c.fill()


def save(path, w, h, scale, draw):
    s = cairo.ImageSurface(cairo.FORMAT_ARGB32, round(w * scale), round(h * scale))
    c = cairo.Context(s)
    c.scale(scale, scale)
    draw(c)
    s.write_to_png(path)


if len(sys.argv) == 3 and sys.argv[1] == "--anchors":
    d = sys.argv[2]
    os.makedirs(d, exist_ok=True)
    save(f"{d}/play.png", 160, 60, 1, play)
    save(f"{d}/play@1.5x.png", 160, 60, 1.5, play)
    save(f"{d}/play@2x.png", 160, 60, 2, play)
    save(f"{d}/stop.png", 160, 60, 1, stop)
    save(f"{d}/item.png", 120, 50, 1, item)
    save(f"{d}/square.png", 100, 100, 1, square)
    save(f"{d}/missing.png", 80, 40, 1, missing)
    sys.exit(0)

state = {"count": 0, "square": False}
win = Gtk.Window(title="Canvas fixture")
win.set_default_size(600, 360)
win.connect("destroy", Gtk.main_quit)
area = Gtk.DrawingArea()
area.set_can_focus(True)
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.KEY_PRESS_MASK)
win.add(area)


def at(c, r, draw):
    c.save()
    c.translate(r[0], r[1])
    draw(c)
    c.restore()


def on_draw(_, c):
    rgb(c, 32, 40, 48)
    c.paint()
    at(c, PLAY, play)
    at(c, STOP, stop)
    at(c, ITEM_A, item)
    at(c, ITEM_B, item)
    if state["square"]:
        at(c, SQUARE, square)
    c.set_source_rgb(1, 1, 1)
    for i in range(min(state["count"], 40)):
        c.rectangle(40 + i * 12, 270, 8, 40)
    c.fill()


def inside(r, x, y):
    return r[0] <= x < r[0] + r[2] and r[1] <= y < r[1] + r[3]


def on_press(_, e):
    if inside(PLAY, e.x, e.y):
        state["count"] += 1
    elif inside(STOP, e.x, e.y):
        state["count"] = 0
    else:
        return
    area.queue_draw()


def on_key(_, e):
    k = Gdk.keyval_name(e.keyval)
    if k == "space":
        state["count"] += 1
    elif k in ("r", "R"):
        state["count"] = 0
    elif k in ("x", "X"):
        state["square"] = not state["square"]
    else:
        return False
    area.queue_draw()
    return True


area.connect("draw", on_draw)
area.connect("button-press-event", on_press)
win.connect("key-press-event", on_key)
win.show_all()
area.grab_focus()
Gtk.main()
