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
};

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
        rows.push(protocol::Row { elements })
            .map_err(|_| "Card の行数が上限を超えます")?;
    }
    let slot = match card.placement {
        api::Placement::Banner => BANNER_FOR_VALIDATION,
        api::Placement::Overlay => protocol::Slot::Overlay,
    };
    device_card(slot, 0, &rows).validate()?;
    Ok(rows)
}

pub fn device_card(slot: protocol::Slot, ttl_s: u16, rows: &Rows) -> protocol::Card {
    protocol::Card {
        slot,
        ttl_s,
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
