//! Модель страницы «Загрузки» (Steam → Загрузки, `/routes/library/downloads`): сводка, формулы размера, секции, карточки.
//!
//! Чистая логика без Slint и без Steam: на входе снимок `Downloads` (steam-bridge) и настройки, на выходе готовые
//! строки по-русски. Источник правил — стоковый клиент (`tools/cdp/build/src/sp.js`, `downloadsStore`),
//! контракт: docs/steam-downloads-page-contract-2026-11-18.md и docs/downloads-page-2026-11-18.md.
//! Секции, статусы и действия берутся из `downloads_menu` (тот же порт стока), чтобы страница и боковое меню не расходились.
use steam_assets::i18n;
use steam_bridge::{DownloadItem, DownloadOverview, DownloadSettings, Downloads, HistorySample};

use super::downloads_menu::{find, item_actions, item_name, percent_of, primary, sections, speed, state_text, status, ItemAction, Op, Section, Status, Txt};

/// Стадия `k_EAppUpdateProgress_Download` в `progress[]` (library.js, модуль 94601). Остальные стадии (проверка, выделение места,
/// распаковка, копирование, коммит) — дисковые и в размер загрузки не входят.
pub const STAGE_DOWNLOAD: usize = 2;

/// Скачано / всего по формуле стока для одной игры (sp.js `H()`/`A()`): активная загрузка берёт числа обзора
/// (`LocalDownloadOverview.progress[2]`), если они уже ненулевые; иначе — сумму `progress[2]` по типам содержимого элемента.
/// `None` — Steam не сообщил размер.
pub fn item_bytes(i: &DownloadItem, ov: Option<&DownloadOverview>) -> Option<(u64, u64)> {
    let live = ov.filter(|o| o.appid == i.appid && i.active).and_then(|o| Some((o.dl_bytes?, o.dl_total?)));
    if let Some(p) = live.filter(|p| p.0 > 0) {
        return Some(p);
    }
    match (i.dl_bytes, i.dl_total) {
        (Some(b), Some(t)) => Some((b, t)),
        // Мост без таблицы стадий: bytes/total уже тоже только стадия 2 (js::DOWNLOADS), ноль = неизвестно.
        _ if i.total > 0 => Some((i.bytes, i.total)),
        _ => None,
    }
}

/// Счётчики игр для строки «Загрузка X из Y» (sp.js модуль 55426 `K()`): `QueuedTransfers` — элементы очереди без расписания,
/// `CompletedTransfers` — завершённые. Это число ИГР, а не байт.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Counts {
    pub queued: usize,
    pub completed: usize,
}

impl Counts {
    pub fn of(d: &Downloads) -> Counts {
        Counts { queued: d.items.iter().filter(|i| i.queue_index != -1 && i.deferred_time == 0).count(), completed: d.items.iter().filter(|i| i.completed).count() }
    }
    /// Номер текущей игры (`Completed + 1`).
    pub fn index(self) -> usize {
        self.completed + 1
    }
    /// Всего игр в пуле (`Queued + Completed`).
    pub fn total(self) -> usize {
        self.queued + self.completed
    }
}

/// Размер очереди. Стоковый Steam байты всей очереди не суммирует; эта сумма — добавка DeckShell, посчитанная
/// из тех же стоковых чисел каждой игры (`item_bytes`), по активной загрузке и «Далее».
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QueueSize {
    /// Сумма «всего» по играм с известным размером.
    pub total: u64,
    /// Сумма уже скачанного (не больше «всего» каждой игры).
    pub done: u64,
    /// Игр в очереди без известного размера (в сумму не входят).
    pub unknown: usize,
}

impl QueueSize {
    pub fn remaining(self) -> u64 {
        self.total - self.done
    }
}

pub fn queue_size(d: &Downloads) -> QueueSize {
    let mut q = QueueSize::default();
    let secs = sections(d);
    for (s, items) in &secs {
        if !matches!(s, Section::Active | Section::Queued) {
            continue;
        }
        for i in items {
            match item_bytes(i, super::downloads_menu::item_overview(i, &d.overview)) {
                Some((b, t)) if t > 0 => {
                    q.total += t;
                    q.done += b.min(t);
                }
                _ => q.unknown += 1,
            }
        }
    }
    q
}

/// Оставшееся время словами: стоковые «Ещё N сек.» / «Ещё N мин.» дополнены часами. Отрицательное — «Подсчёт…».
pub fn eta_words(sec: i64, t: &Txt) -> String {
    if sec < 0 {
        return (t.tr)("#Downloads_State_Calculating", i18n::tr_ref("downloads_page.calculating"));
    }
    match sec {
        0..=60 => i18n::tr_args("downloads_page.sec", &[("sec", &(sec).to_string())]),
        61..=3599 => i18n::tr_args("downloads_page.min", &[("sec", &((sec + 59) / 60).to_string())]),
        _ => i18n::tr_args("downloads_page.hr_min", &[("sec", &(sec / 3600).to_string()), ("sec_2", &((sec % 3600 + 59) / 60 % 60).to_string())]),
    }
}

/// Шапка страницы.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// Заголовок: «Загрузка 2 из 3», «Загрузки приостановлены», «Нет активных загрузок»…
    pub headline: String,
    /// Вторая строка: «Осталось 4,2 Гбайт из 18,0 Гбайт · 3 игры · …».
    pub detail: String,
    /// Скорости: сеть, пик, диск (уже в тексте), пустая строка, если не загружается.
    pub net: String,
    pub peak: String,
    pub disc: String,
    pub counts: Counts,
    pub size: QueueSize,
}

pub fn summary(d: Option<&Downloads>, s: Option<&DownloadSettings>, t: &Txt) -> Summary {
    let bits = s.and_then(|s| s.rate_bits);
    let Some(d) = d else {
        return Summary { headline: i18n::tr_ref("downloads_page.no_data_from_steam").into(), detail: i18n::tr_ref("downloads_page.steam_did_not_hand_over_the_download").into(), net: String::new(), peak: String::new(), disc: String::new(), counts: Counts::default(), size: QueueSize::default() };
    };
    let ov = &d.overview;
    let counts = Counts::of(d);
    let size = queue_size(d);
    let busy = ov.appid != 0 && ov.state != "None" && !ov.state.is_empty();
    let headline = if ov.paused == Some(true) {
        match counts.queued {
            0 => (t.tr)("#BottomBar_DownloadsPaused", i18n::tr_ref("downloads_page.downloads_paused")),
            n => i18n::tr_args("downloads_page.download_paused_queued", &[("count", &(n).to_string())]),
        }
    } else if ov.state == "Downloading" && !ov.is_upload {
        i18n::tr_args("downloads_page.downloading_of", &[("index", &(counts.index()).to_string()), ("total", &(counts.total()).to_string())])
    } else if busy {
        state_text(&ov.state, t)
    } else if counts.total() > 0 && counts.queued == 0 {
        i18n::tr_args("downloads_page.downloads_of_complete", &[("completed", &(counts.completed).to_string()), ("total", &(counts.total()).to_string())])
    } else if counts.queued > 0 {
        i18n::tr_args("downloads_page.in_queue", &[("queued", &(counts.queued).to_string())])
    } else {
        i18n::tr_ref("downloads_page.no_active_downloads").into()
    };
    let mut parts: Vec<String> = Vec::new();
    if size.total > 0 {
        parts.push(i18n::tr_args("downloads_page.of_remaining", &[("remaining", &((t.size)(size.remaining())).to_string()), ("total", &((t.size)(size.total)).to_string())]));
    }
    if size.unknown > 0 {
        parts.push(i18n::tr_plural("downloads_page.unknown_sizes", size.unknown as i64, &[]));
    }
    let games = d.items.iter().filter(|i| !i.completed && i.queue_index != -1 && i.deferred_time == 0).count();
    if games > 0 {
        parts.push(i18n::tr_plural("downloads_page.games_in_queue", games as i64, &[]));
    }
    if busy && ov.paused != Some(true) {
        if let Some(e) = ov.eta_sec {
            parts.push(format!("{}: {}", i18n::tr_ref("downloads_page.current_game"), if e < 0 { eta_words(e, t) } else { i18n::tr_args("downloads_page.about_remaining", &[("eta", &eta_words(e, t))]) }));
        }
    }
    let rate = |v: u64| if v > 0 { speed(v, bits) } else { String::new() };
    let active = busy && ov.paused != Some(true);
    Summary {
        headline,
        detail: parts.join(" · "),
        net: if active { rate(ov.net_bytes_per_sec) } else { String::new() },
        peak: rate(ov.peak_net_bytes_per_sec),
        disc: if active { rate(ov.disc_bytes_per_sec) } else { String::new() },
        counts,
        size,
    }
}

/// Одна карточка списка (и герой активной загрузки).
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub appid: u32,
    pub section: Section,
    pub status: Status,
    pub name: String,
    /// Состояние: «Загрузка данных», «Далее», «В очереди», «Приостановлено», «Ошибка»…
    pub state: String,
    /// Размер/подробность: «3,2 Гбайт / 5,1 Гбайт», «36 Мбайт», «Завершено: …».
    pub size: String,
    /// 0..1 или отрицательное, если полосы нет.
    pub progress: f32,
    pub percent: Option<u32>,
    pub speed: String,
    pub eta: String,
    /// Порядок в очереди (1 = следующая после активной).
    pub order: Option<usize>,
    pub error: bool,
    /// Недостаточно места (update_result 12 или 43).
    pub no_space: bool,
    pub dim: bool,
    pub primary: Option<ItemAction>,
    pub remove: Option<ItemAction>,
    /// Значки типов содержимого (стоковый `Ee`): контент, мастерская, кэш шейдеров.
    pub badges: Vec<Badge>,
}

/// Значок типа содержимого карточки.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// 0 контент игры, 1 мастерская, 2 кэш шейдеров (индекс в `update_type_info`).
    pub kind: u8,
    pub label: String,
    /// 0 ждёт, 1 качается сейчас, 2 завершён.
    pub state: u8,
}

/// Значки по `update_type_info` с `has_update`. Порядок стока: шейдеры, контент, мастерская. «Сейчас» — тип активной загрузки обзора.
pub fn badges(i: &DownloadItem, ov: Option<&DownloadOverview>, t: &Txt) -> Vec<Badge> {
    let live = ov.filter(|o| o.appid == i.appid && i.active);
    let mut v: Vec<Badge> = i
        .types
        .iter()
        .filter(|(k, _)| *k <= 2)
        .map(|&(k, done)| {
            let (tok, fb) = match k {
                1 => ("#Downloads_ContentType_Workshop", i18n::tr_ref("downloads_page.workshop")),
                2 => ("#Downloads_ContentType_Shader", i18n::tr_ref("downloads_page.shader_pre_caching_update")),
                _ => ("#Downloads_ContentType_Content", i18n::tr_ref("downloads_page.content")),
            };
            let now = live.is_some_and(|o| match k {
                1 => o.is_workshop,
                2 => o.is_shader,
                _ => !o.is_workshop && !o.is_shader,
            });
            Badge { kind: k, label: (t.tr)(tok, fb), state: if done { 2 } else if now { 1 } else { 0 } }
        })
        .collect();
    let rank = |k: u8| match k { 2 => 0, 0 => 1, _ => 2 };
    v.sort_by_key(|b| rank(b.kind));
    v
}

/// Окно графика скорости, секунд (стоковое `m = 120`).
pub const GRAPH_SECS: usize = 120;

/// График за последние 2 минуты (стоковый `T`): столбцы сети и линия диска, каждый секундный слот 0..1 от максимума окна.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Graph {
    /// `GRAPH_SECS` столбцов сети, высота 0..1.
    pub net: Vec<f32>,
    /// Тот же ряд для загрузок-отдач (upload) — рисуется другим цветом.
    pub upload: Vec<bool>,
    /// Диск: `GRAPH_SECS` значений 0..1, отрицательное — нет выборки.
    pub disc: Vec<f32>,
    pub max_net: u64,
    pub max_disc: u64,
}

impl Graph {
    pub fn is_empty(&self) -> bool {
        self.max_net == 0 && self.max_disc == 0
    }
}

/// Строит график из `DownloadHistory`: окно — `GRAPH_SECS` секунд, оканчивающиеся последней выборкой (как в стоке).
pub fn graph(h: &[HistorySample]) -> Graph {
    let Some(last) = h.last() else { return Graph::default() };
    let start = last.time.round() - GRAPH_SECS as f64;
    let mut g = Graph { net: vec![0.0; GRAPH_SECS], upload: vec![false; GRAPH_SECS], disc: vec![-1.0; GRAPH_SECS], max_net: 0, max_disc: 0 };
    let mut slots: Vec<Option<&HistorySample>> = vec![None; GRAPH_SECS];
    for s in h.iter().filter(|s| s.time > start) {
        let k = ((s.time.round() - start) as i64 - 1).clamp(0, GRAPH_SECS as i64 - 1) as usize;
        slots[k] = Some(s);
    }
    g.max_net = slots.iter().flatten().map(|s| s.net).max().unwrap_or(0);
    g.max_disc = slots.iter().flatten().map(|s| s.disc).max().unwrap_or(0);
    for (k, s) in slots.iter().enumerate() {
        if let Some(s) = s {
            g.net[k] = if g.max_net > 0 { s.net as f32 / g.max_net as f32 } else { 0.0 };
            g.disc[k] = if g.max_disc > 0 { s.disc as f32 / g.max_disc as f32 } else { 0.0 };
            g.upload[k] = s.upload;
        }
    }
    g
}

/// EAppUpdateError «нет места»: 12 и 43 в локализации Steam (steamui_russian: «Недостаточно свободного места на диске» и вариант про квоты).
/// Контракт Gemini называл так код 14, но в локализации это «Ожидание установочного диска» — сверено со строками клиента.
const NO_SPACE: [i64; 2] = [12, 43];

pub fn card(section: Section, i: &DownloadItem, d: &Downloads, t: &Txt) -> Card {
    let ov = super::downloads_menu::item_overview(i, &d.overview);
    let st = status(i, ov);
    let live = ov.filter(|o| o.appid == i.appid);
    let bytes = item_bytes(i, ov);
    let percent = match live.and_then(|o| o.percent).filter(|_| section == Section::Active) {
        Some(p) => Some(p.round().clamp(0.0, 100.0) as u32),
        None => bytes.and_then(|(b, n)| percent_of(b, n)),
    };
    let code = i.update_result.unwrap_or_default();
    let no_space = st == Status::Error && NO_SPACE.contains(&code);
    // Стадия «Остановка предыдущей загрузки»: элемент активен, а обзор ещё описывает другую игру.
    let all_paused = d.overview.paused == Some(true) && matches!(st, Status::Active | Status::ActiveDownloading | Status::ActivePaused);
    let waiting = !all_paused && matches!(st, Status::Active | Status::ActiveDownloading) && d.overview.appid != 0 && d.overview.appid != i.appid;
    let state = match st {
        _ if all_paused => (t.tr)("#Downloads_State_Paused", i18n::tr_ref("downloads_page.paused")),
        _ if waiting => (t.tr)("#Downloads_State_WaitingForPrevious", i18n::tr_ref("downloads_page.stopping_previous_download")),
        Status::ActiveDownloading | Status::Active => {
            if d.overview.state == "Downloading" && bytes.is_some_and(|(b, n)| n > 0 && b >= n) {
                (t.tr)("#Downloads_DetailedState_Downloading_Complete", i18n::tr_ref("downloads_page.downloading_complete"))
            } else if d.overview.state == "Downloading" {
                (t.tr)("#Downloads_DetailedState_Downloading", i18n::tr_ref("downloads_page.downloading_data"))
            } else {
                state_text(&d.overview.state, t)
            }
        }
        Status::ActivePaused | Status::RemovedFromQueue => (t.tr)("#Downloads_State_Paused", i18n::tr_ref("downloads_page.paused")),
        Status::Queued => (t.tr)("#Downloads_State_Queued", i18n::tr_ref("downloads_page.queued")),
        Status::QueuedNext => (t.tr)("#Downloads_State_UpNext", i18n::tr_ref("downloads_page.next")),
        Status::Scheduled => format!("{} {}", (t.tr)("#Downloads_State_Scheduled", i18n::tr_ref("downloads_page.scheduled")), (t.time)(i.deferred_time)),
        Status::Error if no_space => (t.tr)(&format!("#Steam_AppUpdateError_{code}"), i18n::tr_ref("downloads_page.not_enough_free_disk_space")),
        Status::Error => (t.tr)(&format!("#Steam_AppUpdateError_{code}"), &i18n::tr_args("downloads_page.update_error_code", &[("code", &(code).to_string())])),
        Status::DisabledByUser => (t.tr)("#Downloads_AutoUpdate_DoNotUpdate", i18n::tr_ref("downloads_page.updated_only_at_launch")),
        Status::DisabledByApp => (t.tr)("#Downloads_State_UpdatesDiabledByApp", i18n::tr_ref("downloads_page.the_application_has_temporarily_disabled_updates")),
        Status::Completed => format!("{} {}", (t.tr)("#Downloads_State_Complete", i18n::tr_ref("downloads_page.completed")).trim_end_matches(':'), (t.time)(i.completed_time)).replacen(" ", ": ", 1),
        Status::Unknown => i18n::tr_ref("downloads_page.steam_did_not_report_a_condition").into(),
    };
    let size = if st == Status::Completed {
        bytes.filter(|p| p.1 > 0).map(|p| (t.size)(p.1)).unwrap_or_default()
    } else {
        match bytes {
            Some((b, n)) if b > 0 && n > 0 => format!("{} / {}", (t.size)(b), (t.size)(n)),
            Some((_, n)) if n > 0 => (t.size)(n),
            _ => i18n::tr_ref("downloads_page.size_unknown").into(),
        }
    };
    let active_now = matches!(st, Status::ActiveDownloading | Status::Active) && !waiting && !all_paused;
    let progress = match (st, percent) {
        (Status::Completed, _) => -1.0,
        (_, Some(p)) if bytes.is_some_and(|b| b.0 > 0) || section == Section::Active => p as f32 / 100.0,
        _ => -1.0,
    };
    let order = (section == Section::Queued && i.queue_index >= 1).then_some(i.queue_index as usize);
    let actions = item_actions(d, i.appid, None);
    let primary = primary(section, st, d.overview.paused, i.appid);
    let remove = actions.iter().find(|a| matches!(a.op, Op::Remove { .. })).cloned();
    Card {
        appid: i.appid,
        section,
        status: st,
        name: item_name(i),
        state,
        size,
        progress,
        percent,
        speed: if active_now { live.map(|o| o.net_bytes_per_sec).filter(|&v| v > 0).map(|v| speed(v, None)).unwrap_or_default() } else { String::new() },
        eta: if active_now { live.and_then(|o| o.eta_sec).filter(|&e| e >= 0).map(|e| i18n::tr_args("downloads_page.about_remaining_2", &[("error", &(eta_words(e, t)).to_string())])).unwrap_or_default() } else { String::new() },
        order,
        error: st == Status::Error,
        no_space,
        dim: !active_now,
        primary,
        remove,
        badges: badges(i, ov, t),
    }
}

/// Секция страницы со своим заголовком «Далее (3)».
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub section: Section,
    pub title: String,
    pub cards: Vec<Card>,
}

/// Всё, что рисует страница. Пустые секции не показываются (кроме «Далее» при пустой очереди — там стоит заглушка).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub summary: Summary,
    pub hero: Option<Card>,
    /// График скорости за 2 минуты (для героя); пустой, если Steam не прислал историю.
    pub graph: Graph,
    pub blocks: Vec<Block>,
    /// Подпись кнопки автообновлений в заголовке «Далее».
    pub auto_label: String,
    pub empty: bool,
}

pub fn section_title(s: Section, n: usize, t: &Txt) -> String {
    let (tok, fb) = match s {
        Section::Active => ("#Downloads_Section_Active", i18n::tr_ref("downloads_page.active")),
        Section::Queued => ("#Downloads_Section_Current", i18n::tr_ref("downloads_page.up_next")),
        Section::Scheduled => ("#Downloads_Section_Scheduled", i18n::tr_ref("downloads_page.scheduled")),
        Section::NotQueued => ("#Downloads_Section_NotQueued", i18n::tr_ref("downloads_page.unscheduled")),
        Section::Completed => ("#Downloads_Section_Completed", i18n::tr_ref("downloads_page.completed_2")),
    };
    format!("{} ({n})", (t.tr)(tok, fb))
}

/// Кнопка автообновлений (стоковый `xe`): расписание, если оно включено в настройках, иначе «Автообновления включены».
pub fn auto_label(s: Option<&DownloadSettings>, t: &Txt) -> String {
    let Some(s) = s else { return i18n::tr_ref("downloads_page.loading_settings").into() };
    if s.restrict_auto_updates == Some(true) {
        let hour = |h: Option<i64>| h.and_then(|h| s.hours.iter().find(|x| x.0 == h).map(|x| x.1.clone()));
        if let (Some(a), Some(b)) = (hour(s.restrict_start), hour(s.restrict_end)) {
            return i18n::tr_args("downloads_page.auto_updates_scheduled_from_to", &[("start", &(a).to_string()), ("end", &(b).to_string())]);
        }
        return (t.tr)("#DownloadSettings_ScheduleAutoUpdates", i18n::tr_ref("downloads_page.schedule_auto_updates"));
    }
    (t.tr)("#Downloads_AutoUpdates_Enabled", i18n::tr_ref("downloads_page.auto_updates_enabled"))
}

pub fn page(d: Option<&Downloads>, s: Option<&DownloadSettings>, t: &Txt) -> Page {
    let sum = summary(d, s, t);
    let Some(d) = d else {
        return Page { summary: sum, hero: None, graph: Graph::default(), blocks: vec![], auto_label: auto_label(s, t), empty: true };
    };
    let secs = sections(d);
    let mut hero = None;
    let mut blocks = Vec::new();
    for (sec, items) in &secs {
        let cards: Vec<Card> = items.iter().map(|i| card(*sec, i, d, t)).collect();
        if *sec == Section::Active {
            hero = cards.into_iter().next();
            continue;
        }
        // «Далее» показывается всегда (в стоке — вместе с кнопкой автообновлений); остальные секции — только непустые.
        if cards.is_empty() && *sec != Section::Queued {
            continue;
        }
        blocks.push(Block { section: *sec, title: section_title(*sec, cards.len(), t), cards });
    }
    let empty = hero.is_none() && blocks.iter().all(|b| b.cards.is_empty());
    Page { summary: sum, hero, graph: graph(&d.history), blocks, auto_label: auto_label(s, t), empty }
}

/// Все `appid` страницы в порядке показа (герой, затем секции) — для фокуса.
pub fn order(p: &Page) -> Vec<u32> {
    p.hero.iter().chain(p.blocks.iter().flat_map(|b| b.cards.iter())).map(|c| c.appid).collect()
}

/// Элемент для действий (контекстное меню страницы): все стоковые действия карточки.
pub fn actions(d: &Downloads, appid: u32, s: Option<&DownloadSettings>) -> Vec<ItemAction> {
    item_actions(d, appid, s.and_then(|s| s.throttle_rate))
}

pub fn section_of(d: &Downloads, appid: u32) -> Option<Section> {
    find(d, appid).map(|x| x.0)
}

#[cfg(test)]
#[path = "downloads_page_tests.rs"]
mod tests;
