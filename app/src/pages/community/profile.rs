//! The Profile tab: what friends see of the user. Their game now (NIP-38)
//! with why it is not shared when it is not, the sharing switch, and the
//! profile's name and bio (NIP-01 kind 0). The picture and the rest are
//! in Settings → Community.

use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::social::Profile;

use super::friends::avatar;
use super::CommunityPage;
use crate::pages::{list_item, status_pill};
use crate::services::nostr::{Hidden, MyStatus};
use crate::theme;
use crate::ui::controls::{button_sm, caption, card, help_text, toggle_row, ButtonKind};
use crate::ui::fields::{input_box, labeled, textarea_box};

/// Rows of the bio field.
pub(super) const ABOUT_ROWS: usize = 3;

/// What a profile form was last filled with, so the newest profile can
/// replace it as long as the user has not changed it (a profile from the
/// relays, or one saved in the other form).
#[derive(Default)]
pub(crate) struct Filled(Option<Vec<String>>);

impl Filled {
    /// Whether the form, showing `shown`, takes `current` (the newest
    /// profile's values), which it then counts as filled.
    pub(crate) fn take(&mut self, shown: &[String], current: &[String]) -> bool {
        if shown == current {
            self.0 = Some(current.to_vec());
            return false;
        }
        let untouched = match &self.0 {
            Some(filled) => filled == shown,
            None => shown.iter().all(|s| s.is_empty()),
        };
        if untouched {
            self.0 = Some(current.to_vec());
        }
        untouched
    }
}

/// The name a profile form shows: the display name, else the name.
pub(crate) fn shown_name(p: &Profile) -> String {
    if p.display_name.is_empty() {
        p.name.clone()
    } else {
        p.display_name.clone()
    }
}

/// `p` with the form's name (the display name, and the name when it has
/// none) and bio.
pub(crate) fn with_name_and_about(mut p: Profile, name: &str, about: &str) -> Profile {
    p.display_name = name.trim().to_string();
    if p.name.is_empty() {
        p.name = p.display_name.clone();
    }
    p.about = about.trim().to_string();
    p
}

/// The status line: a pill and a sentence, for the desktop and console.
pub(super) fn status_text(status: &MyStatus) -> (String, bool, String) {
    match status {
        MyStatus::Idle => (tr!("Not playing").into(), false, tr!("When a game runs, it shows here.").into()),
        MyStatus::Shared(title) => (tr!("Playing {game}", game = title.as_str()), true, tr!("Your friends see it.").into()),
        MyStatus::Hidden(title, why) => {
            let why = match why {
                Hidden::SharingOff => tr!("Not shared: \"Share what I'm playing\" is off."),
                Hidden::LeftOut => tr!("Not shared: this game is left out in its Discussions."),
                Hidden::Unknown => {
                    tr!("Not shared: Heronite knows no Steam or IGDB id for this game, so friends' apps could not tell which game it is.")
                }
                Hidden::Offline => tr!("Not shared: the community layer is not connected."),
            };
            (tr!("Playing {game}", game = title.as_str()), false, why.into())
        }
    }
}

impl CommunityPage {
    /// Shows the newest profile in the form unless the user changed it.
    pub(super) fn fill_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.svc.read(cx).my_profile();
        let current = [shown_name(&current), current.about];
        let shown = [self.name.read(cx).value().to_string(), self.about.read(cx).value().to_string()];
        if self.filled.take(&shown, &current) {
            let [name, about] = current;
            self.name.update(cx, |i, cx| i.set_value(name, window, cx));
            self.about.update(cx, |i, cx| i.set_value(about, window, cx));
        }
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let about = self.about.read(cx).value().to_string();
        self.svc.update(cx, |s, cx| {
            let profile = with_name_and_about(s.my_profile(), &name, &about);
            s.save_profile(profile, cx);
        });
    }

    fn status_card(&self, status: &MyStatus, sharing: bool, cx: &mut Context<Self>) -> Div {
        let (head, shared, line) = status_text(status);
        let color = if shared { theme::status_color("playing") } else { theme::status_color("not-played") };
        let pill = if shared { tr!("Shared") } else { tr!("Not shared") };
        let status = list_item(
            None,
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(14.)).text_color(theme::text_primary()).overflow_hidden().text_ellipsis().child(head))
                .child(div().text_size(px(12.5)).text_color(theme::text_muted()).child(line)),
            status_pill(pill, color),
        );
        let share = toggle_row(
            "profile-share-playing",
            tr!("Share what I'm playing"),
            Some(tr!("Friends see the game while it runs, from any store or emulator. A game's menu can leave it out.")),
            sharing,
        )
        .on_click(cx.listener(|this, _, _, cx| this.svc.update(cx, |s, cx| s.set_prefs(|p| p.share_playing = !p.share_playing, cx))));
        div().flex().flex_col().child(caption(tr!("Now"))).child(card([status.into_any_element(), share.into_any_element()]))
    }

    fn profile_card(&self, label: String, npub: String, cx: &mut Context<Self>) -> Div {
        let header = list_item(
            Some(avatar(&label, 44.).into_any_element()),
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(15.)).text_color(theme::text_primary()).overflow_hidden().text_ellipsis().child(label))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_dim())
                        .font_family(theme::FONT_MONO)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(npub.clone()),
                ),
            button_sm("profile-copy-npub", ButtonKind::Ghost, tr!("Copy"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(npub.clone()))),
        );
        let form = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pb(px(12.))
            .child(labeled(tr!("Name"), input_box(&self.name, false)))
            .child(labeled(tr!("Bio"), textarea_box(&self.about, ABOUT_ROWS)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button_sm("profile-save", ButtonKind::Primary, tr!("Save profile"))
                            .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                    )
                    .child(
                        button_sm("profile-more", ButtonKind::Ghost, tr!("Picture and more in Settings"))
                            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                    ),
            )
            .child(help_text(tr!("Your profile is public: every Nostr app shows it.")));
        div().flex().flex_col().child(caption(tr!("Profile"))).child(card([header.into_any_element(), form.into_any_element()]))
    }

    pub(super) fn render_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.fill_profile(window, cx);
        let (status, sharing, label, npub) = {
            let svc = self.svc.read(cx);
            let me = svc.me();
            (
                svc.my_status(),
                svc.prefs().share_playing,
                me.map(|k| svc.label(&k)).unwrap_or_default(),
                me.map(|k| heronite_nostr::identity::npub(&k)).unwrap_or_default(),
            )
        };
        div().flex().flex_col().child(self.status_card(&status, sharing, cx)).child(self.profile_card(label, npub, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{shown_name, with_name_and_about, Filled, Profile};

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_empty_form_takes_the_profile_when_it_comes() {
        let mut f = Filled::default();
        assert!(!f.take(&v(&["", ""]), &v(&["", ""])));
        assert!(f.take(&v(&["", ""]), &v(&["Ana", "Speedruns"])));
        assert!(!f.take(&v(&["Ana", "Speedruns"]), &v(&["Ana", "Speedruns"])));
    }

    #[test]
    fn a_form_being_edited_is_left_alone() {
        let mut f = Filled::default();
        assert!(f.take(&v(&["", ""]), &v(&["Ana", ""])));
        // The user types a bio; a profile from another app comes meanwhile.
        assert!(!f.take(&v(&["Ana", "Ret"]), &v(&["Ana B", ""])));
        // Saved: the form shows what the profile says, and follows it again.
        assert!(!f.take(&v(&["Ana", "Retro"]), &v(&["Ana", "Retro"])));
        assert!(f.take(&v(&["Ana", "Retro"]), &v(&["Ana", "Retro, RPGs"])));
    }

    #[test]
    fn the_form_name_is_the_display_name_and_fills_a_missing_name() {
        let p = with_name_and_about(Profile::default(), " Ana ", " hi ");
        assert_eq!((p.name.as_str(), p.display_name.as_str(), p.about.as_str()), ("Ana", "Ana", "hi"));
        let mut q = Profile::default();
        q.name = "ana".into();
        let q = with_name_and_about(q, "Ana B", "");
        assert_eq!((q.name.as_str(), shown_name(&q).as_str()), ("ana", "Ana B"));
    }
}
