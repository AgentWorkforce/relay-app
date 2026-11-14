"""Finder layout for dmgbuild. Coordinates match render-dmg-background.swift."""
import os

application = defines["app"]
app_name = os.path.basename(application)
format = "UDZO"
filesystem = "HFS+"
files = [application]
symlinks = {"Applications": "/Applications"}
background = defines["background"]
# The window's bounds as Finder records them, title bar included; the
# background is 768x576.
window_rect = ((160, 160), (768, 608))
# The volume's dot files stay out of the window: marked invisible (dmgbuild
# runs SetFile -a V on each that exists, before it writes .DS_Store, so that
# one is never marked), and placed below the visible area for a Finder that
# shows hidden files.
hidden = [".background.tiff", ".DS_Store", ".fseventsd", ".Trashes", ".VolumeIcon.icns"]
hide = hidden
icon_locations = {app_name: (220, 252), "Applications": (548, 252)}
icon_locations.update({name: (100 + 140 * index, 900) for index, name in enumerate(hidden)})
icon_size = 160
text_size = 13
label_pos = "bottom"
default_view = "icon-view"
arrange_by = None
grid_spacing = 80
show_icon_preview = False
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
include_icon_view_settings = True
include_list_view_settings = False
