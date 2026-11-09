//! Remember the window Clack must not steal, and keep the popovers out of the taskbar.
//! GPUI 0.2 has no Linux API to move or hide a window, and no system status item.

#[cfg(target_os = "linux")]
mod linux {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;

    pub struct Watch {
        saved: Option<u32>,
    }

    impl Watch {
        pub fn new() -> Self {
            Self { saved: None }
        }

        pub fn tick(&mut self) {
            let Ok((conn, screen)) = x11rb::connect(None) else {
                return;
            };
            let root = conn.setup().roots[screen].root;
            let Ok(atoms) = Atoms::load(&conn) else {
                return;
            };
            let clients = client_list(&conn, root, atoms.client_list);
            let mut ours = Vec::new();
            for window in &clients {
                let name = window_name(&conn, *window, atoms.net_wm_name, atoms.utf8, atoms.wm_name);
                if name == "Clack" || name == "Clack HUD" {
                    ours.push(*window);
                    nudge(&conn, *window, root, &atoms);
                    place(&conn, *window, root, &name, &atoms);
                }
            }
            if let Some(active) = active_window(&conn, root, atoms.active) {
                if !ours.contains(&active) {
                    self.saved = Some(active);
                }
            }
            let _ = conn.flush();
        }

        pub fn restore(&self) {
            let Some(window) = self.saved else {
                return;
            };
            let Ok((conn, screen)) = x11rb::connect(None) else {
                return;
            };
            let root = conn.setup().roots[screen].root;
            let Ok(active) = intern(&conn, "_NET_ACTIVE_WINDOW") else {
                return;
            };
            let event = ClientMessageEvent {
                response_type: CLIENT_MESSAGE_EVENT,
                format: 32,
                sequence: 0,
                window,
                type_: active,
                data: ClientMessageData::from([1u32, 0, 0, 0, 0]),
            };
            let _ = conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            );
            let _ = conn.flush();
        }
    }

    struct Atoms {
        client_list: Atom,
        active: Atom,
        net_wm_name: Atom,
        utf8: Atom,
        wm_name: Atom,
        state: Atom,
        skip_taskbar: Atom,
        above: Atom,
        motif: Atom,
        moveresize: Atom,
    }

    impl Atoms {
        fn load(conn: &RustConnection) -> Result<Self, ()> {
            Ok(Self {
                client_list: intern(conn, "_NET_CLIENT_LIST")?,
                active: intern(conn, "_NET_ACTIVE_WINDOW")?,
                net_wm_name: intern(conn, "_NET_WM_NAME")?,
                utf8: intern(conn, "UTF8_STRING")?,
                wm_name: intern(conn, "WM_NAME")?,
                state: intern(conn, "_NET_WM_STATE")?,
                skip_taskbar: intern(conn, "_NET_WM_STATE_SKIP_TASKBAR")?,
                above: intern(conn, "_NET_WM_STATE_ABOVE")?,
                motif: intern(conn, "_MOTIF_WM_HINTS")?,
                moveresize: intern(conn, "_NET_MOVERESIZE_WINDOW")?,
            })
        }
    }

    fn intern(conn: &RustConnection, name: &str) -> Result<Atom, ()> {
        conn.intern_atom(false, name.as_bytes())
            .map_err(|_| ())?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|_| ())
    }

    fn client_list(conn: &RustConnection, root: u32, atom: Atom) -> Vec<u32> {
        let Ok(cookie) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 1024) else {
            return Vec::new();
        };
        let Ok(reply) = cookie.reply() else {
            return Vec::new();
        };
        reply.value32().map(|values| values.collect()).unwrap_or_default()
    }

    fn active_window(conn: &RustConnection, root: u32, atom: Atom) -> Option<u32> {
        let reply = conn
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32().and_then(|mut values| values.next())
    }

    fn window_name(
        conn: &RustConnection,
        window: u32,
        net_wm_name: Atom,
        utf8: Atom,
        wm_name: Atom,
    ) -> String {
        if let Some(text) = property_string(conn, window, net_wm_name, utf8) {
            if !text.is_empty() {
                return text;
            }
        }
        property_string(conn, window, wm_name, AtomEnum::STRING.into()).unwrap_or_default()
    }

    fn property_string(conn: &RustConnection, window: u32, property: Atom, type_: Atom) -> Option<String> {
        let reply = conn
            .get_property(false, window, property, type_, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        let bytes: Vec<u8> = reply.value8()?.collect();
        String::from_utf8(bytes).ok()
    }

    fn prop32(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_ne_bytes()).collect()
    }

    fn nudge(conn: &RustConnection, window: u32, root: u32, atoms: &Atoms) {
        let hints = prop32(&[2, 0, 0, 0, 0]);
        let _ = conn.change_property(
            PropMode::REPLACE,
            window,
            atoms.motif,
            atoms.motif,
            32,
            5,
            &hints,
        );
        let state = prop32(&[atoms.skip_taskbar, atoms.above]);
        let _ = conn.change_property(
            PropMode::REPLACE,
            window,
            atoms.state,
            AtomEnum::ATOM,
            32,
            2,
            &state,
        );
        let event = ClientMessageEvent {
            response_type: CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window,
            type_: atoms.state,
            data: ClientMessageData::from([1u32, atoms.skip_taskbar, atoms.above, 0, 0]),
        };
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }

    /// xfwm4 centers `_NET_WM_WINDOW_TYPE_NOTIFICATION` windows and ignores the
    /// position GPUI requested. Ask again, in root coordinates, until it sticks.
    fn place(conn: &RustConnection, window: u32, root: u32, name: &str, atoms: &Atoms) {
        let Ok(cookie) = conn.get_geometry(root) else {
            return;
        };
        let Ok(root_geom) = cookie.reply() else {
            return;
        };
        let screen_w = root_geom.width as i32;
        let (x, y, w, h) = match name {
            "Clack HUD" => {
                let w = 420;
                let h = 112;
                ((screen_w - w) / 2, 28, w, h)
            }
            "Clack" => {
                let w = 320;
                let h = 640;
                (screen_w - w - 12, 28, w, h)
            }
            _ => return,
        };
        let x = x.max(0);
        let y = y.max(0);
        if let Ok(cookie) = conn.translate_coordinates(window, root, 0, 0) {
            let Ok(reply) = cookie.reply() else {
                return;
            };
            let dx = (reply.dst_x as i32 - x).abs();
            let dy = (reply.dst_y as i32 - y).abs();
            if dx <= 8 && dy <= 8 {
                return;
            }
        }
        let _ = conn.configure_window(window, &ConfigureWindowAux::new().x(x).y(y));
        // NorthWest gravity, move x/y and size, source = application.
        let flags: u32 = 1 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);
        let event = ClientMessageEvent {
            response_type: CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window,
            type_: atoms.moveresize,
            data: ClientMessageData::from([flags, x as u32, y as u32, w as u32, h as u32]),
        };
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }
}

#[cfg(target_os = "linux")]
pub use linux::Watch;

#[cfg(not(target_os = "linux"))]
pub struct Watch;

#[cfg(not(target_os = "linux"))]
impl Watch {
    pub fn new() -> Self {
        Self
    }

    pub fn tick(&mut self) {}

    pub fn restore(&self) {}
}
