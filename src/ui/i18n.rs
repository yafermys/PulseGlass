pub struct I18n {
    pub en: bool,
}

impl I18n {
    pub fn new(en: bool) -> Self {
        Self { en }
    }

    pub fn t<'a>(&self, tr: &'a str, en: &'a str) -> &'a str {
        if self.en { en } else { tr }
    }
}
