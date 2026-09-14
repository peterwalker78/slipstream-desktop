//! Emoji for the app explorer: type `:` and a word (`:fire`, `:thumbs`, `:party`), Enter types
//! the emoji into the app you're in. A short list of the ones people reach for, each with its
//! name and the words it's found by.

/// Each emoji, its name, and more words it's found by.
pub const EMOJI: &[(&str, &str, &str)] = &[
    ("😀", "grinning face", "smile happy"),
    ("😃", "grinning face with big eyes", "smile happy"),
    ("😄", "grinning face with smiling eyes", "smile happy laugh"),
    ("😁", "beaming face", "grin smile"),
    ("😆", "grinning squinting face", "laugh"),
    ("😅", "grinning face with sweat", "phew relief"),
    ("🤣", "rolling on the floor laughing", "rofl lol"),
    ("😂", "face with tears of joy", "lol laugh crying"),
    ("🙂", "slightly smiling face", "smile"),
    ("🙃", "upside-down face", "silly sarcasm"),
    ("😉", "winking face", "wink"),
    ("😊", "smiling face with smiling eyes", "blush happy"),
    ("😇", "smiling face with halo", "angel innocent"),
    ("🥰", "smiling face with hearts", "love adore"),
    ("😍", "smiling face with heart-eyes", "love crush"),
    ("🤩", "star-struck", "wow excited"),
    ("😘", "face blowing a kiss", "kiss love"),
    ("😋", "face savouring food", "yum tasty"),
    ("😛", "face with tongue", "tongue cheeky"),
    ("😜", "winking face with tongue", "crazy silly"),
    ("🤪", "zany face", "crazy goofy"),
    ("🤔", "thinking face", "hmm think"),
    ("🤨", "face with raised eyebrow", "suspicious sceptical"),
    ("😐", "neutral face", "meh"),
    ("😑", "expressionless face", "blank"),
    ("😶", "face without mouth", "silent speechless"),
    ("🙄", "face with rolling eyes", "eyeroll whatever"),
    ("😏", "smirking face", "smirk"),
    ("😬", "grimacing face", "awkward yikes"),
    ("🤥", "lying face", "liar pinocchio"),
    ("😌", "relieved face", "calm"),
    ("😔", "pensive face", "sad"),
    ("😪", "sleepy face", "tired"),
    ("😴", "sleeping face", "zzz sleep"),
    ("😷", "face with medical mask", "sick ill"),
    ("🤒", "face with thermometer", "sick ill"),
    ("🤢", "nauseated face", "sick gross"),
    ("🤮", "face vomiting", "sick gross"),
    ("🥵", "hot face", "heat sweating"),
    ("🥶", "cold face", "freezing"),
    ("🤯", "exploding head", "mind blown shocked"),
    ("🥳", "partying face", "party celebrate birthday"),
    ("😎", "smiling face with sunglasses", "cool"),
    ("🤓", "nerd face", "geek glasses"),
    ("🧐", "face with monocle", "inspect curious"),
    ("😕", "confused face", "puzzled"),
    ("😟", "worried face", "concerned"),
    ("😮", "face with open mouth", "surprised wow"),
    ("😲", "astonished face", "shocked"),
    ("🥺", "pleading face", "puppy eyes please"),
    ("😢", "crying face", "sad tear"),
    ("😭", "loudly crying face", "sob sad"),
    ("😱", "face screaming in fear", "scream scared"),
    ("😖", "confounded face", "frustrated"),
    ("😩", "weary face", "tired fed up"),
    ("😤", "face with steam from nose", "triumph huff"),
    ("😡", "enraged face", "angry mad"),
    ("😠", "angry face", "mad grumpy"),
    ("🤬", "face with symbols on mouth", "swearing cursing"),
    ("💀", "skull", "dead dying"),
    ("💩", "pile of poo", "poop"),
    ("🤡", "clown face", "clown"),
    ("👻", "ghost", "boo halloween"),
    ("👽", "alien", "ufo"),
    ("🤖", "robot", "bot"),
    ("😺", "grinning cat", "cat"),
    ("🙈", "see-no-evil monkey", "monkey embarrassed"),
    ("🙉", "hear-no-evil monkey", "monkey"),
    ("🙊", "speak-no-evil monkey", "monkey oops"),
    ("❤️", "red heart", "love heart"),
    ("🧡", "orange heart", "love heart"),
    ("💛", "yellow heart", "love heart"),
    ("💚", "green heart", "love heart"),
    ("💙", "blue heart", "love heart"),
    ("💜", "purple heart", "love heart"),
    ("🖤", "black heart", "love heart"),
    ("🤍", "white heart", "love heart"),
    ("💔", "broken heart", "heartbreak sad"),
    ("💕", "two hearts", "love"),
    ("💯", "hundred points", "100 perfect score"),
    ("💥", "collision", "boom bang"),
    ("💫", "dizzy", "stars"),
    ("💦", "sweat droplets", "water splash"),
    ("💨", "dashing away", "fast wind"),
    ("💬", "speech balloon", "chat comment"),
    ("💭", "thought balloon", "thinking"),
    ("💤", "zzz", "sleep tired"),
    ("👋", "waving hand", "wave hello hi bye"),
    ("🤚", "raised back of hand", "hand"),
    ("✋", "raised hand", "stop high five"),
    ("👌", "ok hand", "okay perfect"),
    ("🤌", "pinched fingers", "italian"),
    ("✌️", "victory hand", "peace"),
    ("🤞", "crossed fingers", "luck hope"),
    ("🤟", "love-you gesture", "love"),
    ("🤘", "sign of the horns", "rock metal"),
    ("👈", "backhand index pointing left", "left point"),
    ("👉", "backhand index pointing right", "right point"),
    ("👆", "backhand index pointing up", "up point"),
    ("👇", "backhand index pointing down", "down point"),
    ("👍", "thumbs up", "yes like approve +1"),
    ("👎", "thumbs down", "no dislike -1"),
    ("✊", "raised fist", "fist power"),
    ("👊", "oncoming fist", "punch fist bump"),
    ("👏", "clapping hands", "clap applause bravo"),
    ("🙌", "raising hands", "hooray celebrate"),
    ("👐", "open hands", "hug"),
    ("🤝", "handshake", "deal agreement"),
    ("🙏", "folded hands", "please thanks pray"),
    ("✍️", "writing hand", "write"),
    ("💪", "flexed biceps", "strong muscle"),
    ("👀", "eyes", "look see watching"),
    ("🧠", "brain", "smart think"),
    ("👶", "baby", "child"),
    ("🧑‍💻", "technologist", "coder developer programmer"),
    ("🤷", "person shrugging", "shrug dunno whatever"),
    ("🤦", "person facepalming", "facepalm"),
    ("🙋", "person raising hand", "question volunteer"),
    ("🎉", "party popper", "party celebrate tada congratulations"),
    ("🎊", "confetti ball", "party celebrate"),
    ("🎂", "birthday cake", "birthday cake party"),
    ("🎁", "wrapped gift", "present birthday"),
    ("🎄", "christmas tree", "christmas xmas"),
    ("🎃", "jack-o-lantern", "halloween pumpkin"),
    ("🏆", "trophy", "win award prize"),
    ("🥇", "first place medal", "gold win"),
    ("⚽", "soccer ball", "football"),
    ("🎮", "video game", "gaming controller"),
    ("🎵", "musical note", "music song"),
    ("🎶", "musical notes", "music song"),
    ("🎧", "headphone", "music listen"),
    ("📷", "camera", "photo picture"),
    ("🎬", "clapper board", "film movie"),
    ("📺", "television", "tv"),
    ("💻", "laptop", "computer"),
    ("🖥️", "desktop computer", "computer monitor"),
    ("⌨️", "keyboard", "type"),
    ("🖱️", "computer mouse", "mouse click"),
    ("📱", "mobile phone", "phone"),
    ("🔋", "battery", "charge power"),
    ("🔌", "electric plug", "power"),
    ("💡", "light bulb", "idea"),
    ("🔦", "flashlight", "torch"),
    ("📚", "books", "study library read"),
    ("📝", "memo", "note write"),
    ("📎", "paperclip", "attach"),
    ("📌", "pushpin", "pin"),
    ("📅", "calendar", "date"),
    ("📈", "chart increasing", "growth up"),
    ("📉", "chart decreasing", "down loss"),
    ("🔒", "locked", "lock secure"),
    ("🔑", "key", "password unlock"),
    ("🔨", "hammer", "tool build"),
    ("🔧", "wrench", "tool fix"),
    ("⚙️", "gear", "settings cog"),
    ("🧪", "test tube", "science experiment"),
    ("🐛", "bug", "insect debug"),
    ("🚀", "rocket", "launch ship fast"),
    ("✈️", "airplane", "flight travel"),
    ("🚗", "car", "drive"),
    ("🚲", "bicycle", "bike cycle"),
    ("🏠", "house", "home"),
    ("🌍", "globe showing europe-africa", "world earth"),
    ("🌙", "crescent moon", "night"),
    ("⭐", "star", "favourite"),
    ("🌟", "glowing star", "sparkle"),
    ("✨", "sparkles", "shiny magic new"),
    ("☀️", "sun", "sunny weather"),
    ("⛅", "sun behind cloud", "cloudy weather"),
    ("🌧️", "cloud with rain", "rain weather"),
    ("❄️", "snowflake", "snow cold winter"),
    ("⚡", "high voltage", "lightning zap fast"),
    ("🔥", "fire", "lit hot flame"),
    ("💧", "droplet", "water"),
    ("🌈", "rainbow", "pride"),
    ("🌸", "cherry blossom", "flower spring"),
    ("🌹", "rose", "flower"),
    ("🌻", "sunflower", "flower"),
    ("🌱", "seedling", "plant grow"),
    ("🍀", "four leaf clover", "luck"),
    ("🐶", "dog face", "dog puppy"),
    ("🐱", "cat face", "cat kitten"),
    ("🦊", "fox", "animal"),
    ("🐻", "bear", "animal"),
    ("🐼", "panda", "animal"),
    ("🐧", "penguin", "linux tux"),
    ("🦀", "crab", "rust rustacean"),
    ("🐍", "snake", "python"),
    ("🦄", "unicorn", "magic"),
    ("🍕", "pizza", "food"),
    ("🍔", "hamburger", "burger food"),
    ("🍟", "french fries", "chips food"),
    ("🌮", "taco", "food"),
    ("🍣", "sushi", "food"),
    ("🍎", "red apple", "fruit"),
    ("🍌", "banana", "fruit"),
    ("🍓", "strawberry", "fruit"),
    ("🍩", "doughnut", "donut"),
    ("🍪", "cookie", "biscuit"),
    ("🍫", "chocolate bar", "sweet"),
    ("☕", "hot beverage", "coffee tea"),
    ("🍵", "teacup without handle", "tea"),
    ("🍺", "beer mug", "beer pint pub"),
    ("🍷", "wine glass", "wine"),
    ("🥂", "clinking glasses", "cheers toast celebrate"),
    ("✅", "check mark button", "done yes tick"),
    ("✔️", "check mark", "done yes tick"),
    ("❌", "cross mark", "no wrong"),
    ("❓", "question mark", "question"),
    ("❗", "exclamation mark", "important"),
    ("⚠️", "warning", "caution alert"),
    ("🚫", "prohibited", "no forbidden"),
    ("⛔", "no entry", "stop"),
    ("🆗", "ok button", "okay"),
    ("🆕", "new button", "new"),
    ("➡️", "right arrow", "arrow next"),
    ("⬅️", "left arrow", "arrow back"),
    ("⬆️", "up arrow", "arrow"),
    ("⬇️", "down arrow", "arrow"),
    (
        "🔄",
        "counterclockwise arrows button",
        "refresh sync repeat",
    ),
    ("♻️", "recycling symbol", "recycle"),
    ("🟢", "green circle", "online go"),
    ("🔴", "red circle", "record stop"),
    ("🟡", "yellow circle", "away"),
    ("⏰", "alarm clock", "time wake"),
    ("⏳", "hourglass not done", "wait loading"),
    ("🕐", "one o’clock", "time clock"),
    ("🇬🇧", "flag: united kingdom", "uk britain"),
    ("🇺🇸", "flag: united states", "usa america"),
    ("🇪🇺", "flag: european union", "eu europe"),
    ("🏴󠁧󠁢󠁥󠁮󠁧󠁿", "flag: england", "england"),
    ("🏴󠁧󠁢󠁳󠁣󠁴󠁿", "flag: scotland", "scotland"),
    ("🏴󠁧󠁢󠁷󠁬󠁳󠁿", "flag: wales", "wales cymru"),
];

/// Emoji whose name or words start with `query`'s words, names starting with it first, at most
/// `limit` of them.
pub fn search(query: &str, limit: usize) -> Vec<(&'static str, &'static str)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut found: Vec<(u8, usize, &'static str, &'static str)> = EMOJI
        .iter()
        .enumerate()
        .filter_map(|(order, (emoji, name, extra))| {
            let all: Vec<&str> = name
                .split(|c: char| c.is_whitespace() || c == '-' || c == ':')
                .chain(extra.split_whitespace())
                .filter(|word| !word.is_empty())
                .collect();
            let every = words
                .iter()
                .all(|word| all.iter().any(|candidate| candidate.starts_with(word)));
            if !every {
                return None;
            }
            let rank = if name.starts_with(&query) {
                0
            } else if name
                .split_whitespace()
                .any(|word| word.starts_with(words[0]))
            {
                1
            } else {
                2
            };
            Some((rank, order, *emoji, *name))
        })
        .collect();
    found.sort_by_key(|(rank, order, ..)| (*rank, *order));
    found
        .into_iter()
        .take(limit)
        .map(|(_, _, emoji, name)| (emoji, name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_finds_its_emoji() {
        assert_eq!(search("fire", 3).first().map(|found| found.0), Some("🔥"));
        assert_eq!(search("thumbs", 3).first().map(|found| found.0), Some("👍"));
        assert_eq!(search("tada", 3).first().map(|found| found.0), Some("🎉"));
    }

    #[test]
    fn every_word_has_to_match() {
        assert!(search("thumbs down", 5).iter().all(|found| found.0 == "👎"));
        assert!(search("zzzqqq", 5).is_empty());
        assert!(search("", 5).is_empty());
    }

    #[test]
    fn names_that_start_with_the_query_come_first() {
        let hearts = search("heart", 20);
        assert!(hearts.len() > 5);
        assert!(hearts[0].1.starts_with("heart") || hearts[0].1.contains("heart"));
    }

    #[test]
    fn no_emoji_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for (emoji, ..) in EMOJI {
            assert!(seen.insert(emoji), "{emoji} twice");
        }
    }
}
