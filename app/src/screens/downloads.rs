//! /routes/library/downloads — страница «Загрузки». Данные и формулы: `downloads_page` (модель), вид: ui/screens/downloads.slint.
//! Здесь — раскладка строк, фокус, прокрутка и действия. Источник данных один — снимок Steam через мост (`live.downloads`):
//! страница ничего не дописывает от себя, после записи в Steam она ждёт следующий снимок (как боковое меню загрузок).
use steam_assets::i18n;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, VecModel};
use steam_bridge::art::{ArtResolver, Mapped};

use crate::app::{fmt_size, App};
use crate::assets::t_or;
use crate::input::Action;
use crate::loader::Kind;
use crate::router::Route;
use crate::screens::downloads_menu::{local_time, Op, Section, Txt};
use crate::screens::downloads_page::{self, Badge, Card, Graph, Page};
use crate::{AppWindow, DlBadge, DlPageRow, DownloadsState};

/// Высоты строк в пикселях окна (1280×800). Карточка очереди — 83 css-px стока × 1,2 ≈ 100 с полями.
const H_SUMMARY: f32 = 130.0;
/// Баннер героя 360 px под верхним баром плюс отступ до первого заголовка секции.
const H_HERO: f32 = 370.0;
const H_HEADER: f32 = 70.0;
const H_ITEM: f32 = 112.0;
const H_EMPTY: f32 = 56.0;
/// Видимая высота списка под верхней панелью.
const VIEWPORT: f32 = 747.5;

/// Что делает нажатие на кнопку заголовка секции.
#[derive(Clone, Debug, PartialEq)]
pub enum HeaderAct {
    /// «Автообновления включены» → настройки загрузок.
    AutoUpdates,
    /// «Скрыть все» в «Завершено»: RemoveFromDownloadList каждой игры.
    ClearCompleted(Vec<u32>),
}

/// Одна строка страницы: что рисуется и что на ней можно нажать. Кнопка 1 — основное действие, 2 — «убрать».
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub kind: i32,
    pub appid: u32,
    pub y: f32,
    pub h: f32,
    pub name: String,
    pub state: String,
    pub size: String,
    pub speed: String,
    /// Осталось примерно… (только герой).
    pub eta: String,
    pub pct: String,
    pub progress: f32,
    pub order: i32,
    pub tone: i32,
    pub b1: String,
    pub b1_icon: i32,
    pub b2: String,
    pub hb: String,
    pub primary: Option<Op>,
    pub remove: Option<Op>,
    pub header: Option<HeaderAct>,
    pub badges: Vec<Badge>,
    /// График скорости (только у героя).
    pub graph: Option<Graph>,
    /// Карточку можно перетащить (очередь «Далее» с номером).
    pub drag: bool,
}

impl Line {
    fn new(kind: i32, h: f32) -> Line {
        Line { kind, appid: 0, y: 0.0, h, name: String::new(), state: String::new(), size: String::new(), speed: String::new(), eta: String::new(), pct: String::new(), progress: -1.0, order: 0, tone: 0, b1: String::new(), b1_icon: 0, b2: String::new(), hb: String::new(), primary: None, remove: None, header: None, badges: vec![], graph: None, drag: false }
    }
    /// Можно ли встать на строку.
    pub fn focusable(&self) -> bool {
        matches!(self.kind, 1 | 3) || (self.kind == 2 && self.header.is_some())
    }
    /// Доступные кнопки строки (0 карточка / заголовок, 1 действие, 2 убрать).
    pub fn cols(&self) -> Vec<i32> {
        let mut v = vec![0];
        if self.primary.is_some() {
            v.push(1);
        }
        if self.remove.is_some() {
            v.push(2);
        }
        v
    }
}

/// Подпись и значок основной кнопки (1 пауза, 2 продолжить, 3 загрузить, 4 повторить, 0 без значка).
fn primary_label(c: &Card) -> Option<(String, i32, Op)> {
    let a = c.primary.as_ref()?;
    let (label, icon) = match a.op {
        Op::EnableAll(false) => (i18n::tr_ref("downloads.pause"), 1),
        Op::EnableAll(true) => (i18n::tr_ref("downloads.continue"), 2),
        Op::DownloadNow(_) if c.error => (i18n::tr_ref("downloads.retry"), 4),
        Op::DownloadNow(_) if c.section == Section::Queued => (i18n::tr_ref("downloads.download_first"), 3),
        Op::DownloadNow(_) => (i18n::tr_ref("downloads.load"), 3),
        Op::ViewInLibrary(_) => (i18n::tr_ref("downloads.to_play"), 0),
        _ => return None,
    };
    Some((label.into(), icon, a.op))
}

fn tone(c: &Card) -> i32 {
    if c.error {
        2
    } else if c.dim {
        3
    } else {
        1
    }
}

fn fill(l: &mut Line, c: &Card, remove_label: &str) {
    l.appid = c.appid;
    l.name = c.name.clone();
    l.state = c.state.clone();
    l.size = c.size.clone();
    l.tone = tone(c);
    l.progress = c.progress;
    l.badges = c.badges.clone();
    l.pct = c.percent.filter(|_| c.progress >= 0.0).map(|p| format!("{p}%")).unwrap_or_default();
    if let Some((label, icon, op)) = primary_label(c) {
        l.b1 = label;
        l.b1_icon = icon;
        l.primary = Some(op);
    }
    if let Some(a) = &c.remove {
        l.b2 = remove_label.into();
        l.remove = Some(a.op);
    }
}

/// Раскладка страницы сверху вниз: сводка, «Сейчас загружается», секции, примечание. Чистая функция, проверяется тестами.
pub fn layout(p: &Page) -> Vec<Line> {
    // как в стоке: сверху нет заголовка — сразу блок «Сейчас загружается»; строка статуса только когда блока нет
    let mut v = if p.hero.is_none() && !p.summary.headline.is_empty() { vec![Line::new(0, H_SUMMARY)] } else { vec![] };
    if let Some(h) = &p.hero {
        let mut l = Line::new(1, H_HERO);
        fill(&mut l, h, i18n::tr_ref("downloads.remove"));
        l.graph = Some(p.graph.clone()).filter(|g| !g.is_empty());
        l.eta = h.eta.clone();
        l.speed = [h.speed.clone(), h.eta.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        v.push(l);
    }
    for b in &p.blocks {
        let mut head = Line::new(2, H_HEADER);
        head.name = b.title.clone();
        match b.section {
            Section::Queued => {
                head.hb = p.auto_label.to_uppercase();
                head.header = Some(HeaderAct::AutoUpdates);
            }
            Section::Completed if !b.cards.is_empty() => {
                head.hb = i18n::tr_ref("downloads.hide_everything").into();
                head.header = Some(HeaderAct::ClearCompleted(b.cards.iter().map(|c| c.appid).collect()));
            }
            _ => {}
        }
        v.push(head);
        if b.cards.is_empty() {
            let mut l = Line::new(4, H_EMPTY);
            l.name = if p.hero.is_some() { i18n::tr_ref("downloads.there_s_nothing_else_in_line").into() } else { i18n::tr_ref("downloads.there_are_no_downloads_in_the_queue").into() };
            v.push(l);
        }
        for c in &b.cards {
            let mut l = Line::new(3, H_ITEM);
            fill(&mut l, c, "×");
            l.order = c.order.map(|o| o as i32).unwrap_or(0);
            l.drag = b.section == Section::Queued && c.order.is_some();
            v.push(l);
        }
    }
    let mut y = 0.0;
    for l in v.iter_mut() {
        l.y = y;
        y += l.h;
    }
    v
}

/// Ломаная диска в системе координат графика 120×100 (SVG-путь для Slint `Path.commands`); отрезки рвутся там, где нет выборки.
pub fn disc_path(g: &Graph) -> String {
    let mut out = String::new();
    let mut pen = false;
    for (k, &d) in g.disc.iter().enumerate() {
        if d < 0.0 {
            pen = false;
            continue;
        }
        let y = 99.0 - d.clamp(0.0, 1.0) * 98.0;
        out += &format!("{}{:.1} {:.1} ", if pen { "L" } else { "M" }, k as f32 + 0.5, y);
        pen = true;
    }
    out
}

/// Конец перетаскивания карточки очереди: сдвиг в пикселях → новый номер в очереди (`SetQueueIndex`), границы — номера «Далее».
pub fn drag_target(lines: &[Line], row: usize, dy: f32) -> Option<(u32, u32)> {
    let me = lines.get(row).filter(|l| l.drag)?;
    let orders: Vec<i32> = lines.iter().filter(|l| l.drag).map(|l| l.order).collect();
    let (lo, hi) = (*orders.iter().min()?, *orders.iter().max()?);
    let to = (me.order + (dy / H_ITEM).round() as i32).clamp(lo, hi);
    (to != me.order).then_some((me.appid, to as u32))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DownloadArtwork {
    appid: u32,
    steam_root: PathBuf,
    account: Option<u32>,
    icon: Option<PathBuf>,
    capsule: Option<PathBuf>,
}

const PORTRAIT_SIZE: u32 = 96;

fn remote_icon(url: &str) -> Option<image::DynamicImage> {
    const MAX_BYTES: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    ureq::get(url)
        .timeout(Duration::from_secs(3))
        .call().ok()?
        .into_reader().take(MAX_BYTES + 1)
        .read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    reader.limits(limits);
    reader.decode().ok()
}

impl DownloadArtwork {
    fn load(&self) -> Option<(SharedPixelBuffer<Rgba8Pixel>, bool)> {
        let resolver = ArtResolver::new(&self.steam_root, self.account.unwrap_or_default());
        let cached = steam_data::art::librarycache(&resolver.cache_dir(), &self.appid.to_string());
        // The stock header asks Steam for its icon asset, including the hashed
        // cache path and CDN URL. Resolve on this worker even for apps omitted
        // from the visible library (for example, a tool or shared content).
        let urls = crate::actions::bridge()
            .and_then(|bridge| bridge.app_icon_urls(self.appid).ok())
            .unwrap_or_default();
        self.decode(&resolver, &cached, &urls)
    }

    fn decode(
        &self,
        resolver: &ArtResolver,
        cached: &steam_data::Art,
        urls: &[String],
    ) -> Option<(SharedPixelBuffer<Rgba8Pixel>, bool)> {
        let local = |path: PathBuf| {
            image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()
        };
        let mut remote = None;
        let steam_icon = urls.iter().find_map(|url| match resolver.map_url(url) {
            Mapped::Local(path, _) => local(path),
            Mapped::Remote(url) if url.starts_with("https://") => {
                remote.get_or_insert(url);
                None
            }
            _ => None,
        });
        let icon = steam_icon
            .or_else(|| remote.as_deref().and_then(remote_icon))
            .or_else(|| self.icon.clone().and_then(local))
            .or_else(|| cached.icon.clone().and_then(local));
        let (picture, is_capsule) = match icon {
            Some(icon) => (icon, false),
            None => (self.capsule.clone().and_then(local)
                .or_else(|| cached.capsule.clone().and_then(local))?, true),
        };
        let picture = if picture.width().max(picture.height()) > PORTRAIT_SIZE {
            picture.thumbnail(PORTRAIT_SIZE, PORTRAIT_SIZE)
        } else {
            picture
        };
        let pixels = picture.into_rgba8();
        Some((SharedPixelBuffer::clone_from_slice(pixels.as_raw(), pixels.width(), pixels.height()), is_capsule))
    }
}

pub struct State {
    pub page: Option<Page>,
    pub lines: Vec<Line>,
    /// Строка с фокусом (индекс в `lines`).
    pub focus: usize,
    /// Кнопка в строке: 0 карточка, 1 действие, 2 убрать.
    pub col: i32,
    pub vy: f32,
    pub timer: Timer,
    model: Rc<VecModel<DlPageRow>>,
    artwork: Option<DownloadArtwork>,
    artwork_generation: u64,
}
impl Default for State {
    fn default() -> Self {
        State { page: None, lines: vec![], focus: 0, col: 0, vy: 0.0, timer: Timer::default(), model: Rc::new(VecModel::default()), artwork: None, artwork_generation: 0 }
    }
}

/// Строка, на которую встаёт фокус при заходе на страницу: герой или первая карточка.
fn first_focus(lines: &[Line]) -> usize {
    lines.iter().position(|l| l.kind == 1 || l.kind == 3).or_else(|| lines.iter().position(Line::focusable)).unwrap_or(0)
}

/// Следующая строка с фокусом в направлении `dir` (−1 вверх, +1 вниз); None — край.
pub fn step_row(lines: &[Line], from: usize, dir: i32) -> Option<usize> {
    let mut i = from as i32 + dir;
    while i >= 0 && (i as usize) < lines.len() {
        if lines[i as usize].focusable() {
            return Some(i as usize);
        }
        i += dir;
    }
    None
}

pub fn wire(ui: &AppWindow) {
    let state = ui.global::<DownloadsState>();
    state.set_label_disk_usage(crate::screens::stock_label("#Downloads_Legend_Disk", "downloads.disk_usage").into());
    state.set_label_installing_files(crate::screens::stock_label("#Downloads_DetailedState_Installing", "downloads.installing_files").into());
    state.set_label_managing_downloads_for(crate::screens::stock_label("#Downloads_ManageDownloadsFor", "downloads.managing_downloads_for").into());
    state.set_label_network(crate::screens::stock_label("#Downloads_Legend_Network", "downloads.network").into());
    state.set_label_peak(crate::screens::stock_label("#Downloads_Stats_PeakRate", "downloads.peak").into());
    state.set_label_this_device(crate::screens::stock_label("#Downloads_ThisDeviceUppercase", "downloads.this_device").into());
    ui.global::<DownloadsState>().on_drag_end(|row, dy| super::defer(move |a| {
        if let Some((appid, to)) = drag_target(&a.dl.lines, row as usize, dy) {
            a.dl_run(Op::QueueIndex(appid, to));
        }
    }));
    ui.global::<DownloadsState>().on_activate(|row, col| super::defer(move |a| {
        if a.dl.lines.get(row as usize).is_some_and(Line::focusable) {
            a.dl.focus = row as usize;
            a.dl.col = col;
            a.dl_press();
        }
    }));
}

pub fn wire_models(app: &App, ui: &AppWindow) {
    ui.global::<DownloadsState>().set_rows(ModelRc::from(app.dl.model.clone()));
}

pub fn init_strings(_ui: &AppWindow) {}

impl App {
    /// Данные Steam изменились (или страницу открыли): перестроить страницу из последнего снимка.
    pub fn on_downloads_data(&mut self) {
        self.dl_menu_refresh();
        if let Some(d) = self.dl_menu_data() {
            for i in &d.items {
                if let Some(&gi) = self.gidx.get(&i.appid) {
                    self.request(gi, Kind::Header);
                    if i.active {
                        self.request(gi, Kind::Hero);
                        self.request(gi, Kind::Logo);
                    }
                }
            }
        }
        if self.dm.settings.is_none() && self.router.current() == &Route::Downloads {
            self.dl_menu_load_settings();
        }
        self.rebuild_dl();
    }

    /// Бывший опрос файловой системы при недоступном мосте убран: страница показывает только то, что сообщил Steam.
    pub fn start_downloads_poll(&mut self) {}

    /// Пришла картинка игры: перерисовать страницу и верхний бар, если это игра активной загрузки.
    pub fn rebuild_dl_art(&mut self, id: u32) {
        if self.dl.page.as_ref().and_then(|p| p.hero.as_ref()).is_some_and(|h| h.appid == id) {
            self.rebuild_dl();
        }
    }

    pub fn rebuild_dl(&mut self) {
        let d = self.dl_menu_data();
        let settings = self.dm.settings.clone().flatten();
        let txt = Txt { tr: &t_or, time: &local_time, size: &fmt_size };
        let page = downloads_page::page(d.as_ref(), settings.as_ref(), &txt);
        // курсор остаётся на той же игре и кнопке, когда Steam меняет порядок
        let keep = self.dl.lines.get(self.dl.focus).filter(|l| l.appid != 0).map(|l| (l.appid, l.kind));
        let lines = layout(&page);
        let same = keep.and_then(|(id, k)| lines.iter().position(|l| l.appid == id && l.kind == k));
        let fresh = same.is_none() && (self.dl.lines.is_empty() || !lines.get(self.dl.focus).is_some_and(Line::focusable));
        self.dl.focus = same.unwrap_or_else(|| if fresh { first_focus(&lines) } else { self.dl.focus });
        // как в стоке: на герое сразу кнопка паузы (A приостанавливает загрузку)
        if fresh && lines.get(self.dl.focus).is_some_and(|l| l.kind == 1 && l.primary.is_some()) {
            self.dl.col = 1;
        }
        if !lines.get(self.dl.focus).is_some_and(|l| l.cols().contains(&self.dl.col)) {
            self.dl.col = 0;
        }
        self.dl.lines = lines;
        self.dl.page = Some(page);
        self.push_dl();
    }

    fn push_dl(&mut self) {
        let total = self.dl.lines.last().map(|l| l.y + l.h).unwrap_or(0.0);
        if let Some(l) = self.dl.lines.get(self.dl.focus) {
            // верхняя строка показывает сводку целиком; иначе строка целиком в окне
            let top = if self.dl.lines[..self.dl.focus].iter().all(|x| !x.focusable()) { 0.0 } else { l.y };
            if top < self.dl.vy {
                self.dl.vy = top;
            } else if l.y + l.h > self.dl.vy + VIEWPORT {
                self.dl.vy = l.y + l.h - VIEWPORT;
            }
        }
        self.dl.vy = self.dl.vy.clamp(0.0, (total - VIEWPORT).max(0.0));
        let rows: Vec<DlPageRow> = self.dl.lines.iter().map(|l| {
            let thumb = self.imgs.get(&(l.appid, Kind::Header));
            let hero_art = self.imgs.get(&(l.appid, Kind::Hero)).filter(|_| l.kind == 1);
            let logo = self.imgs.get(&(l.appid, Kind::Logo)).filter(|_| l.kind == 1);
            // герой: «скачано / всего» двумя частями; заголовок секции: «Далее» и «(3)» разными кеглями
            let (done, total) = if l.kind == 1 { l.size.split_once(" / ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default() } else { Default::default() };
            let (name, size) = match (l.kind, l.name.rfind(" (")) {
                (2, Some(k)) => (l.name[..k].to_string(), l.name[k + 1..].to_string()),
                (2, None) => (l.name.clone(), String::new()),
                _ => (l.name.clone(), l.size.clone()),
            };
            DlPageRow {
                done: done.into(), total: total.into(), eta: l.eta.as_str().into(),
                hero: hero_art.cloned().unwrap_or_default(), has_hero: hero_art.is_some(),
                logo: logo.cloned().unwrap_or_default(), has_logo: logo.is_some(),
                kind: l.kind, y: l.y, h: l.h, name: name.as_str().into(), state: l.state.as_str().into(), size: size.as_str().into(),
                speed: l.speed.as_str().into(), pct: l.pct.as_str().into(), progress: l.progress,
                thumb: thumb.cloned().unwrap_or_default(), has_thumb: l.appid != 0 && thumb.is_some(),
                order: l.order, tone: l.tone, drag: l.drag,
                badges: ModelRc::from(Rc::new(VecModel::from(l.badges.iter().map(|b| DlBadge { kind: b.kind as i32, label: b.label.as_str().into(), state: b.state as i32 }).collect::<Vec<_>>()))),
                has_graph: l.graph.is_some(),
                bars: ModelRc::from(Rc::new(VecModel::from(l.graph.as_ref().map(|g| g.net.clone()).unwrap_or_default()))),
                disc_path: l.graph.as_ref().map(disc_path).unwrap_or_default().into(),
                max_net: l.graph.as_ref().filter(|g| g.max_net > 0).map(|g| super::downloads_menu::speed(g.max_net, None)).unwrap_or_default().into(),
                b1: l.b1.as_str().into(), b1_icon: l.b1_icon, b2: l.b2.as_str().into(), hb: l.hb.as_str().into(),
            }
        }).collect();
        self.dl.model.set_vec(rows);
        let ui = self.ui();
        let st = ui.global::<DownloadsState>();
        let (head, detail, net, peak, disc) = match &self.dl.page {
            Some(p) => (p.summary.headline.clone(), p.summary.detail.clone(), p.summary.net.clone(), p.summary.peak.clone(), p.summary.disc.clone()),
            None => Default::default(),
        };
        self.push_dl_indicator();
        let ui = self.ui();
        let st = ui.global::<DownloadsState>();
        st.set_headline(head.into());
        st.set_detail(detail.into());
        st.set_net(net.into());
        st.set_peak(peak.into());
        st.set_disc(disc.into());
        st.set_content_h(total.into());
        st.set_vy((-self.dl.vy).into());
        st.set_focus_row(self.dl.focus as i32);
        st.set_focus_col(self.dl.col);
        self.update_hint();
    }

    /// Верхний бар: иконка загружаемой игры и тонкая полоса прогресса, пока в Steam идёт (или стоит на паузе) активная загрузка.
    fn push_dl_indicator(&mut self) {
        let ui = self.ui();
        let tb = ui.global::<crate::TopBarState>();
        let hero = self.dl.page.as_ref().and_then(|p| p.hero.as_ref());
        tb.set_dl_active(hero.is_some());
        tb.set_dl_progress(hero.map_or(-1.0, |h| h.progress));
        tb.set_dl_muted(hero.is_some_and(|h| h.error || h.dim));
        let artwork = hero.map(|h| {
            let art = self.gidx.get(&h.appid).map(|&gi| &self.games()[gi].art);
            DownloadArtwork {
                appid: h.appid,
                steam_root: self.base.steam_root.clone(),
                account: self.base.active_user,
                icon: art.and_then(|a| a.icon.clone()),
                capsule: art.and_then(|a| a.capsule.clone()),
            }
        });
        if artwork.is_none() && super::chrome::hf() == 6 {
            self.header_clear();
        }
        if self.dl.artwork == artwork {
            return;
        }
        tb.set_dl_has_icon(false);
        tb.set_dl_icon(Image::default());
        self.dl.artwork_generation = self.dl.artwork_generation.wrapping_add(1);
        let generation = self.dl.artwork_generation;
        self.dl.artwork = artwork;
        if let Some(artwork) = self.dl.artwork.clone() {
            std::thread::spawn(move || {
                let Some((pixels, is_capsule)) = artwork.load() else { return };
                super::defer(move |app| {
                    if app.dl.artwork_generation == generation {
                        let ui = app.ui();
                        let tb = ui.global::<crate::TopBarState>();
                        tb.set_dl_icon(Image::from_rgba8(pixels));
                        tb.set_dl_icon_is_capsule(is_capsule);
                        tb.set_dl_has_icon(true);
                    }
                });
            });
        }
    }

    pub fn handle_downloads(&mut self, a: Action) {
        let Some(line) = self.dl.lines.get(self.dl.focus).cloned() else {
            if a == Action::Y {
                self.dl_toggle_all();
            }
            return;
        };
        match a {
            Action::Up | Action::Down => {
                let dir = if a == Action::Up { -1 } else { 1 };
                if let Some(i) = step_row(&self.dl.lines, self.dl.focus, dir) {
                    self.dl.focus = i;
                    if !self.dl.lines[i].cols().contains(&self.dl.col) {
                        self.dl.col = 0;
                    }
                }
            }
            Action::Left | Action::Right => {
                let cols = line.cols();
                let at = cols.iter().position(|&c| c == self.dl.col).unwrap_or(0);
                let to = if a == Action::Left { at.saturating_sub(1) } else { (at + 1).min(cols.len() - 1) };
                self.dl.col = cols[to];
            }
            Action::A => return self.dl_press(),
            // X: основное действие карточки (пауза / продолжить / загрузить первым / повторить)
            Action::X => {
                if let Some(op) = line.primary {
                    self.dl_run(op);
                }
                return;
            }
            // Y: «Сменить устройство» — у DeckShell одно управляемое устройство, Steam других не сообщает
            Action::Y => return self.toast(i18n::tr_ref("downloads.downloads_of_other_devices_are_not_available")),
            Action::Menu => return self.dl_context_menu(),
            _ => return,
        }
        self.push_dl();
    }

    /// A на кнопке или карточке в фокусе.
    fn dl_press(&mut self) {
        let Some(line) = self.dl.lines.get(self.dl.focus).cloned() else { return };
        match (line.kind, self.dl.col) {
            (2, _) => match line.header {
                Some(HeaderAct::AutoUpdates) => self.navigate(Route::Settings("downloads".into())),
                Some(HeaderAct::ClearCompleted(ids)) => self.dl_clear_completed(&ids),
                None => {}
            },
            (_, 1) => {
                if let Some(op) = line.primary {
                    self.dl_run(op);
                }
            }
            (_, 2) => {
                if let Some(op) = line.remove {
                    self.dl_run(op);
                }
            }
            _ => self.dl_run(Op::ViewInLibrary(line.appid)),
        }
        self.push_dl();
    }

    /// Y: приостановить / возобновить все загрузки по паузе, которую сообщил Steam; неизвестная пауза — ничего не отправляется.
    fn dl_toggle_all(&mut self) {
        match self.live.downloads.as_ref().and_then(|d| d.overview.paused) {
            Some(p) => self.dl_run(Op::EnableAll(p)),
            None => self.toast(i18n::tr_ref("downloads.steam_did_not_say_if_downloads_were")),
        }
    }

    /// Одно действие очереди: запись в Steam и ожидание его следующего снимка (без подмены локального состояния).
    pub fn dl_run(&mut self, op: Op) {
        if let Op::ViewInLibrary(id) = op {
            match self.gidx.get(&id) {
                Some(&gi) => self.open_game(gi),
                None => self.toast(i18n::tr_ref("downloads.the_game_is_not_in_the_deckshell")),
            }
            return;
        }
        if let Some(js) = op.js() {
            self.dl_menu_write("download", js);
        }
    }

    /// «Скрыть все»: убрать из списка каждую завершённую игру одним запросом.
    fn dl_clear_completed(&mut self, ids: &[u32]) {
        if ids.is_empty() {
            return;
        }
        let calls: Vec<String> = ids.iter().filter_map(|&appid| Op::Remove { appid, reenable: false }.js()).collect();
        self.dl_menu_write("clear-completed", format!("[{}]", calls.join(",")));
    }

    /// START на карточке: все стоковые действия игры (включая «Выше» и «Ниже» в очереди).
    fn dl_context_menu(&mut self) {
        use super::library::{MItem, MenuAct, MenuKind};
        let Some(line) = self.dl.lines.get(self.dl.focus).filter(|l| l.appid != 0).cloned() else { return };
        let Some(d) = self.dl_menu_data() else { return };
        let settings = self.dm.settings.clone().flatten();
        let it = |label: String, act: MenuAct| MItem { label, kind: 0, sep: false, arrow: false, act };
        let mut items: Vec<MItem> = downloads_page::actions(&d, line.appid, settings.as_ref())
            .into_iter()
            .map(|a| it(if a.token.is_empty() { a.fallback.to_string() } else { t_or(a.token, a.fallback) }, MenuAct::Dl(a.op)))
            .collect();
        if items.is_empty() {
            return;
        }
        items.push(it(t_or("#Button_Cancel", i18n::tr_ref("downloads.cancel")), MenuAct::Cancel));
        self.open_custom_menu(MenuKind::Download, line.name.clone(), items, 0);
    }

    /// Действие из контекстного меню.
    pub fn dl_op(&mut self, op: Op) {
        self.dl_run(op);
    }
}

#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
