//! plugin API の型からデバイスプロトコルの型への変換。
//!
//! plugin API は firmware から独立に版を持つため、表示領域・文字列長などのデバイス固有の
//! 制約はここで検証する。変換に失敗した内容はデバイスへ送らず、plugin へ理由を返す。

use plugin_api as api;

/// Card の検証に使う帯の高さは BannerTop と BannerBottom で同じである。
const BANNER_FOR_VALIDATION: protocol::Slot = protocol::Slot::BannerTop;

pub const LIMITS: api::Limits = api::Limits {
    // 帯 48 px = 上下余白 8 px + 20 px 行 2 本。Overlay は Card の行数上限で決まる。
    banner_rows: 2,
    overlay_rows: protocol::MAX_CARD_ROWS as u8,
    row_elements: protocol::MAX_ROW_ELEMENTS as u8,
    text_bytes: protocol::MAX_CARD_TEXT_BYTES as u16,
    bar_label_bytes: protocol::MAX_BAR_LABEL_BYTES as u16,
    notify_bytes: protocol::MAX_TEXT_BYTES as u16,
    presence_detail_bytes: protocol::MAX_STATUS_DETAIL_BYTES as u16,
    image_width: protocol::MAX_IMAGE_REGION_WIDTH,
    image_height: protocol::MAX_IMAGE_REGION_HEIGHT,
};

/// 画像の大きさがデバイスの画像領域に収まることを確かめる。
pub fn image_frame(frame: &api::ImageFrame) -> Result<(), &'static str> {
    if frame.width > protocol::MAX_IMAGE_REGION_WIDTH
        || frame.height > protocol::MAX_IMAGE_REGION_HEIGHT
    {
        return Err("画像がデバイスの画像領域より大きいです");
    }
    Ok(())
}

/// 画像を画面の中央に置き、デバイスへ送るメッセージの列にする。行は `ImageRows` 1 件に
/// 収まるだけまとめる。
pub fn image_messages(
    id: u16,
    ttl_s: u16,
    width: u16,
    height: u16,
    pixels: &[u8],
) -> Vec<protocol::Message> {
    let begin = protocol::ImageBegin {
        id,
        x: (protocol::SCREEN_WIDTH - width) / 2,
        y: (protocol::SCREEN_HEIGHT - height) / 2,
        width,
        height,
        ttl_s,
    };
    let row_bytes = usize::from(width) * 2;
    let rows_per_message = (protocol::MAX_IMAGE_ROWS_BYTES / row_bytes).max(1);
    let mut messages = vec![protocol::Message::ImageBegin(begin)];
    for (index, chunk) in pixels.chunks(row_bytes * rows_per_message).enumerate() {
        messages.push(protocol::Message::ImageRows(protocol::ImageRows {
            id,
            row: (index * rows_per_message) as u16,
            pixels: heapless::Vec::from_slice(chunk).expect("行の組は上限に収まる"),
        }));
    }
    messages.push(protocol::Message::ImageEnd { id });
    messages
}

pub type Rows = heapless::Vec<protocol::Row, { protocol::MAX_CARD_ROWS }>;

/// Card の行をデバイスの型へ変換し、置き先の領域に収まることを確かめる。
pub fn card_rows(card: &api::CardPut) -> Result<Rows, &'static str> {
    let mut rows = Rows::new();
    for row in &card.rows {
        let mut elements = heapless::Vec::new();
        for element in &row.elements {
            let element = match element {
                api::Element::Text { text } => protocol::Element::Text {
                    text: text.as_str().try_into().map_err(|_| "Text が長すぎます")?,
                },
                api::Element::Bar { ratio, label } => protocol::Element::Bar {
                    ratio: *ratio,
                    label: label
                        .as_str()
                        .try_into()
                        .map_err(|_| "ラベルが長すぎます")?,
                },
                api::Element::Spacer { height } => protocol::Element::Spacer { height: *height },
            };
            elements
                .push(element)
                .map_err(|_| "行の要素数が上限を超えます")?;
        }
        rows.push(protocol::Row {
            elements,
            action: row.action,
        })
        .map_err(|_| "Card の行数が上限を超えます")?;
    }
    let slot = match card.placement {
        api::Placement::Banner => BANNER_FOR_VALIDATION,
        api::Placement::Overlay => protocol::Slot::Overlay,
    };
    device_card(slot, 0, 0, &rows).validate()?;
    Ok(rows)
}

pub fn device_card(slot: protocol::Slot, ttl_s: u16, id: u16, rows: &Rows) -> protocol::Card {
    protocol::Card {
        slot,
        ttl_s,
        id,
        rows: rows.clone(),
        image: None,
    }
}

pub fn notice_text(
    text: &str,
) -> Result<heapless::String<{ protocol::MAX_TEXT_BYTES }>, &'static str> {
    text.try_into().map_err(|_| "通知が長すぎます")
}

fn expression(value: api::Expression) -> protocol::Expression {
    use api::Expression as A;
    use protocol::Expression as P;
    match value {
        A::Happy => P::Happy,
        A::Focused => P::Focused,
        A::Sleepy => P::Sleepy,
        A::Worried => P::Worried,
        A::Surprised => P::Surprised,
        A::Grin => P::Grin,
        A::Calm => P::Calm,
        A::Curious => P::Curious,
        A::Playful => P::Playful,
        A::Wink => P::Wink,
        A::Sad => P::Sad,
        A::Determined => P::Determined,
    }
}

fn gaze(value: api::Gaze) -> protocol::Gaze {
    match value {
        api::Gaze::Center => protocol::Gaze::Center,
        api::Gaze::Left => protocol::Gaze::Left,
        api::Gaze::Right => protocol::Gaze::Right,
        api::Gaze::Up => protocol::Gaze::Up,
        api::Gaze::Down => protocol::Gaze::Down,
        api::Gaze::Point { x, y } => protocol::Gaze::Point { x, y },
    }
}

fn eyes(value: api::EyeStyle) -> protocol::EyeStyle {
    match value {
        api::EyeStyle::Auto => protocol::EyeStyle::Auto,
        api::EyeStyle::Open => protocol::EyeStyle::Open,
        api::EyeStyle::Wide => protocol::EyeStyle::Wide,
        api::EyeStyle::Closed => protocol::EyeStyle::Closed,
        api::EyeStyle::HalfLidded => protocol::EyeStyle::HalfLidded,
    }
}

fn activity(value: api::Activity) -> protocol::Activity {
    match value {
        api::Activity::Idle => protocol::Activity::Idle,
        api::Activity::Working => protocol::Activity::Working,
        api::Activity::Waiting => protocol::Activity::Waiting,
        api::Activity::Done => protocol::Activity::Done,
        api::Activity::Error => protocol::Activity::Error,
    }
}

pub fn presence(value: &api::Presence) -> Result<protocol::Presence, &'static str> {
    let presence = protocol::Presence {
        activity: value.activity.map(activity),
        detail: value
            .detail
            .as_str()
            .try_into()
            .map_err(|_| "詳細が長すぎます")?,
        expression: expression(value.expression),
        gaze: gaze(value.gaze),
        eyes: eyes(value.eyes),
        ttl_s: value.ttl_s,
    };
    presence.gaze.validate()?;
    Ok(presence)
}

/// `motion` が許可されていない場合は強度を 0 にし、画面上の表情だけを変える。
pub fn emote(value: &api::Emote, motion: bool) -> Result<protocol::Emote, &'static str> {
    let emote = protocol::Emote {
        expression: expression(value.expression),
        gaze: gaze(value.gaze),
        eyes: eyes(value.eyes),
        intensity: if motion { value.intensity } else { 0 },
        duration_ms: value.duration_ms,
    };
    emote.validate()?;
    Ok(emote)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(placement: api::Placement, rows: usize, text: &str) -> api::CardPut {
        api::CardPut {
            card: 0,
            placement,
            priority: api::Priority::Normal,
            ttl_s: 0,
            rows: vec![
                api::Row {
                    elements: vec![api::Element::Text { text: text.into() }],
                    action: None,
                };
                rows
            ],
        }
    }

    #[test]
    fn banner_accepts_two_rows_and_overlay_accepts_card_limit() {
        assert!(card_rows(&card(api::Placement::Banner, 2, "12:34")).is_ok());
        assert!(card_rows(&card(api::Placement::Banner, 3, "12:34")).is_err());
        assert!(
            card_rows(&card(
                api::Placement::Overlay,
                usize::from(LIMITS.overlay_rows),
                "x"
            ))
            .is_ok()
        );
    }

    #[test]
    fn device_string_limits_are_enforced() {
        let at_limit = "x".repeat(usize::from(LIMITS.text_bytes));
        assert!(card_rows(&card(api::Placement::Banner, 1, &at_limit)).is_ok());
        assert!(card_rows(&card(api::Placement::Banner, 1, &format!("{at_limit}x"))).is_err());
        let mut presence = api::Presence {
            activity: Some(api::Activity::Working),
            detail: "x".repeat(usize::from(LIMITS.presence_detail_bytes)),
            expression: api::Expression::Focused,
            gaze: api::Gaze::Center,
            eyes: api::EyeStyle::Auto,
            ttl_s: 30,
        };
        assert!(super::presence(&presence).is_ok());
        presence.detail.push('x');
        assert!(super::presence(&presence).is_err());
    }

    #[test]
    fn images_are_centered_and_split_into_frames_that_fit() {
        let pixels: Vec<u8> = (0..160 * 120 * 2).map(|i| i as u8).collect();
        let messages = image_messages(3, 5, 160, 120, &pixels);
        let protocol::Message::ImageBegin(begin) = messages[0] else {
            panic!("最初は ImageBegin");
        };
        assert_eq!(
            (begin.x, begin.y, begin.width, begin.height),
            (80, 60, 160, 120)
        );
        assert_eq!(begin.validate(), Ok(()));
        assert_eq!(messages.len(), 1 + 40 + 1, "160 px は 1 件 3 行");
        let mut reassembled = Vec::new();
        for (index, message) in messages[1..messages.len() - 1].iter().enumerate() {
            let protocol::Message::ImageRows(rows) = message else {
                panic!("中間は ImageRows");
            };
            assert_eq!(rows.row, (index * 3) as u16);
            reassembled.extend_from_slice(&rows.pixels);
            let mut buffer = [0; protocol::MAX_FRAME_BYTES];
            assert!(
                protocol::encode(message, &mut buffer).is_ok(),
                "フレームに収まる"
            );
        }
        assert_eq!(reassembled, pixels);
        assert_eq!(
            messages.last(),
            Some(&protocol::Message::ImageEnd { id: 3 })
        );
        // 端数の行と、1 行が 1 件を超えない幅。
        assert_eq!(image_messages(0, 0, 7, 5, &[0; 70]).len(), 3);
        assert!(
            image_frame(&api::ImageFrame {
                width: 161,
                height: 1,
                ttl_s: 0,
                pixels: vec![]
            })
            .is_err()
        );
    }

    #[test]
    fn emote_without_motion_does_not_drive_actuators() {
        let request = api::Emote {
            expression: api::Expression::Happy,
            gaze: api::Gaze::Point { x: 10, y: -10 },
            eyes: api::EyeStyle::Auto,
            intensity: 80,
            duration_ms: 1000,
        };
        assert_eq!(emote(&request, false).unwrap().intensity, 0);
        assert_eq!(emote(&request, true).unwrap().intensity, 80);
    }
}
