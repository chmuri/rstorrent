use rt_tui::display::{BaseWindow, Frame, Window, EXTENT_FULL, FLAG_ACTIVE, FLAG_BOTTOM};
use std::sync::{Arc, Mutex};

#[test]
fn test_standard_three_row_layout_80x24() {
    // Header: 1 row static
    let header_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 80, 1, EXTENT_FULL, 1)));
    let header = Frame::with_window(header_win.clone());

    // Main: dynamic height
    let main_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 80, 5, EXTENT_FULL, EXTENT_FULL)));
    let main = Frame::with_window(main_win.clone());

    // Status: 1 row static with FLAG_BOTTOM
    let status_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE | FLAG_BOTTOM, 80, 1, EXTENT_FULL, 1)));
    let status = Frame::with_window(status_win.clone());

    let mut root = Frame::with_row(vec![header, main, status]);
    root.balance(0, 0, 80, 24);

    {
        let h = header_win.lock().unwrap();
        assert_eq!(h.x, 0);
        assert_eq!(h.y, 0);
        assert_eq!(h.width, 80);
        assert_eq!(h.height, 1);
        assert!(!h.is_offscreen());
    }

    {
        let m = main_win.lock().unwrap();
        assert_eq!(m.x, 0);
        assert_eq!(m.y, 1);
        assert_eq!(m.width, 80);
        assert_eq!(m.height, 22);
        assert!(!m.is_offscreen());
    }

    {
        let s = status_win.lock().unwrap();
        assert_eq!(s.x, 0);
        assert_eq!(s.y, 23);
        assert_eq!(s.width, 80);
        assert_eq!(s.height, 1);
        assert!(!s.is_offscreen());
    }
}

#[test]
fn test_standard_three_row_layout_120x40() {
    let header_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 120, 1, EXTENT_FULL, 1)));
    let header = Frame::with_window(header_win.clone());

    let main_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 120, 5, EXTENT_FULL, EXTENT_FULL)));
    let main = Frame::with_window(main_win.clone());

    let status_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE | FLAG_BOTTOM, 120, 1, EXTENT_FULL, 1)));
    let status = Frame::with_window(status_win.clone());

    let mut root = Frame::with_row(vec![header, main, status]);
    root.balance(0, 0, 120, 40);

    {
        let h = header_win.lock().unwrap();
        assert_eq!(h.x, 0);
        assert_eq!(h.y, 0);
        assert_eq!(h.width, 120);
        assert_eq!(h.height, 1);
    }

    {
        let m = main_win.lock().unwrap();
        assert_eq!(m.x, 0);
        assert_eq!(m.y, 1);
        assert_eq!(m.width, 120);
        assert_eq!(m.height, 38);
    }

    {
        let s = status_win.lock().unwrap();
        assert_eq!(s.x, 0);
        assert_eq!(s.y, 39);
        assert_eq!(s.width, 120);
        assert_eq!(s.height, 1);
    }
}

#[test]
fn test_tight_resolution_10x5() {
    let header_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 10, 1, EXTENT_FULL, 1)));
    let header = Frame::with_window(header_win.clone());

    let main_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 10, 1, EXTENT_FULL, EXTENT_FULL)));
    let main = Frame::with_window(main_win.clone());

    let status_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE | FLAG_BOTTOM, 10, 1, EXTENT_FULL, 1)));
    let status = Frame::with_window(status_win.clone());

    let mut root = Frame::with_row(vec![header, main, status]);
    root.balance(0, 0, 10, 5);

    {
        let h = header_win.lock().unwrap();
        assert_eq!(h.x, 0);
        assert_eq!(h.y, 0);
        assert_eq!(h.width, 10);
        assert_eq!(h.height, 1);
    }

    {
        let m = main_win.lock().unwrap();
        assert_eq!(m.x, 0);
        assert_eq!(m.y, 1);
        assert_eq!(m.width, 10);
        assert_eq!(m.height, 3);
    }

    {
        let s = status_win.lock().unwrap();
        assert_eq!(s.x, 0);
        assert_eq!(s.y, 4);
        assert_eq!(s.width, 10);
        assert_eq!(s.height, 1);
    }
}

#[test]
fn test_column_layout_split() {
    // Left column dynamic 20..100
    let left_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 20, 24, EXTENT_FULL, 24)));
    let left = Frame::with_window(left_win.clone());

    // Right column static 30
    let right_win = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 30, 24, 30, 24)));
    let right = Frame::with_window(right_win.clone());

    let mut root = Frame::with_column(vec![left, right]);
    root.balance(0, 0, 80, 24);

    {
        let l = left_win.lock().unwrap();
        assert_eq!(l.x, 0);
        assert_eq!(l.width, 50);
        assert_eq!(l.height, 24);
    }

    {
        let r = right_win.lock().unwrap();
        assert_eq!(r.x, 50);
        assert_eq!(r.width, 30);
        assert_eq!(r.height, 24);
    }
}

#[test]
fn test_inactive_and_zero_size_is_offscreen() {
    let win = Arc::new(Mutex::new(BaseWindow::new(0, 10, 5, 20, 10))); // not active
    let mut frame = Frame::with_window(win.clone());

    frame.balance(0, 0, 80, 24);
    assert!(win.lock().unwrap().is_offscreen());

    let win_active = Arc::new(Mutex::new(BaseWindow::new(FLAG_ACTIVE, 10, 5, 20, 10)));
    let mut frame_active = Frame::with_window(win_active.clone());

    frame_active.balance(0, 0, 0, 24); // zero width
    assert!(win_active.lock().unwrap().is_offscreen());
}
