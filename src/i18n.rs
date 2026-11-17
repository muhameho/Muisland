//! Interface languages. English strings are the lookup keys, so untranslated
//! text gracefully falls back to English.

pub const LANGUAGES: [&str; 2] = ["English", "العربية"];

pub fn t(language: usize, en: &'static str) -> &'static str {
    if language == 1 {
        arabic(en).unwrap_or(en)
    } else {
        en
    }
}

fn arabic(en: &str) -> Option<&'static str> {
    Some(match en {
        // Navigation and structure
        "Accounts" => "الحسابات",
        "Account details" => "تفاصيل الحساب",
        "New account" => "حساب جديد",
        "Settings" => "الإعدادات",
        "Add" => "إضافة",
        "Add account" => "إضافة حساب",
        "Name" => "الاسم",
        "Plan" => "الخطة",
        "Active" => "نشط",
        "Saved" => "محفوظ",
        "Save" => "حفظ",
        "Save name" => "حفظ الاسم",
        "Save account" => "حفظ الحساب",
        "Remove" => "حذف",
        "Discard" => "تجاهل",
        "Cancel" => "إلغاء",
        "Got it" => "تمام",
        "Unavailable" => "غير متاحة",
        "1 account" => "حساب واحد",
        "{n} accounts" => "{n} حسابات",
        "Your account" => "حسابك",
        "another account" => "حساب آخر",
        "No linked account" => "لا يوجد حساب مرتبط",
        // Usage
        "5-hour" => "٥ ساعات",
        "Weekly" => "أسبوعي",
        "Session" => "الجلسة",
        "Extended" => "ممتد",
        "5H" => "٥س",
        "7D" => "٧ي",
        "SES" => "جلسة",
        "EXT" => "ممتد",
        "MAX" => "الأعلى",
        "Not available" => "غير متاح",
        "Reset time unavailable" => "وقت التجديد غير معروف",
        "Reset due · refreshing" => "حان التجديد · جاري التحديث",
        "Resets in {t}" => "يتجدد بعد {t}",
        "{d}d {h}h" => "{d}ي {h}س",
        "{h}h {m}m" => "{h}س {m}د",
        "{m}m" => "{m}د",
        "just now" => "الآن",
        "{n}m ago" => "منذ {n}د",
        "{n}h ago" => "منذ {n}س",
        "{n}d ago" => "منذ {n}ي",
        "Usage not available yet" => "الاستخدام غير متاح بعد",
        "Live · updated {age}" => "مباشر · آخر تحديث {age}",
        "Waiting for usage…" => "في انتظار بيانات الاستخدام…",
        "Usage updates paused" => "تحديثات الاستخدام متوقفة",
        "Working on your account…" => "جاري العمل على حسابك…",
        "Preview · sample data" => "معاينة · بيانات تجريبية",
        "Preview · sample values" => "معاينة · قيم تجريبية",
        "Active · sample data" => "نشط · بيانات تجريبية",
        "Saved · sample data" => "محفوظ · بيانات تجريبية",
        " · stale" => " · قديمة",
        "Not saved · Save to link" => "غير محفوظ · اضغط حفظ للربط",
        "Draft · enter a name, then save" => "مسودة · اكتب اسمًا ثم احفظ",
        "Usage unavailable · see details" => "الاستخدام غير متاح · راجع التفاصيل",
        "Saved · reading usage…" => "محفوظ · جاري قراءة الاستخدام…",
        "Saved · usage not yet read" => "محفوظ · لم تتم قراءة الاستخدام بعد",
        "Updates automatically from your Claude session." => "يتحدث تلقائيًا من جلسة Claude.",
        "Updates through Codex app-server using file credentials." => "يتحدث عبر خادم Codex باستخدام بيانات الدخول المحفوظة.",
        "Codex switches sign-ins; open sessions keep their work." => "Codex يبدّل تسجيل الدخول فقط؛ الجلسات المفتوحة تحتفظ بعملها.",
        // Empty state and hover card
        "Your island starts here." => "جزيرتك تبدأ من هنا.",
        "Add an account, then save its sign-in." => "أضف حسابًا ثم احفظ تسجيل دخوله.",
        "Click to open" => "اضغط للفتح",
        "Switched · usage refreshes shortly" => "تم التبديل · سيتحدث الاستخدام قريبًا",
        // Settings
        "APPEARANCE" => "المظهر",
        "Match Windows" => "مثل ويندوز",
        "FLOATING ISLAND" => "الجزيرة العائمة",
        "Style" => "الشكل",
        "Circle" => "دائرة",
        "Pill" => "كبسولة",
        "Center shows" => "يعرض في المنتصف",
        "Percentage" => "النسبة",
        "Logo" => "الشعار",
        "Percentage of" => "نسبة",
        "Highest" => "الأعلى",
        "Island size" => "حجم الجزيرة",
        "Hover card size" => "حجم البطاقة",
        "Menu size" => "حجم القائمة",
        "LANGUAGE" => "اللغة",
        "BEHAVIOR" => "السلوك",
        "Reduce motion" => "تقليل الحركة",
        "Start with Windows" => "التشغيل مع ويندوز",
        "Tray icon" => "أيقونة شريط المهام",
        "Live usage ring in the notification area" => "حلقة استخدام مباشرة بجوار الساعة",
        "Keyboard shortcut" => "اختصار لوحة المفاتيح",
        "Ctrl + Alt + M opens Muisland" => "Ctrl + Alt + M يفتح Muisland",
        "ALERTS & AUTO-SWITCH" => "التنبيهات والتبديل التلقائي",
        "Notify at usage thresholds" => "نبّهني عند حدود الاستخدام",
        "Session limit" => "حد الجلسة",
        "Weekly limit" => "الحد الأسبوعي",
        "Auto-switch at these limits" => "بدّل تلقائيًا عند هذه الحدود",
        "Ask first when work may be open" => "اسأل أولًا لو فيه شغل مفتوح",
        "Auto-switch waits 15 seconds. Cancel or close the Island to stop it. Open apps may have running tasks; idle detection is not guaranteed." => "التبديل التلقائي ينتظر ١٥ ثانية. ألغِه أو أغلق الجزيرة لإيقافه. قد تكون هناك مهام تعمل في التطبيقات المفتوحة؛ اكتشاف الخمول غير مضمون.",
        "SAFETY & RECOVERY" => "الأمان والاستعادة",
        "Back up local data before switch" => "نسخ احتياطي للبيانات قبل التبديل",
        "Encrypted checkpoint while Claude is closed" => "نقطة استعادة مشفرة أثناء إغلاق Claude",
        "Restore previous sign-in" => "استعادة تسجيل الدخول السابق",
        "Recover local backup to folder" => "استرجاع النسخة الاحتياطية إلى مجلد",
        "Quit Muisland" => "إنهاء Muisland",
        "Unofficial · not affiliated with Anthropic or OpenAI" => "غير رسمي · لا يتبع Anthropic أو OpenAI",
        // Tray menu
        "Open Muisland" => "فتح Muisland",
        "Close Muisland" => "إغلاق Muisland",
        "Show floating island" => "إظهار الجزيرة العائمة",
        "Minimize to tray" => "تصغير إلى منطقة الإشعارات",
        "When off, Muisland stays in the tray" => "عند الإيقاف يبقى Muisland في منطقة الإشعارات",
        "Muisland is in the tray" => "Muisland في منطقة الإشعارات",
        "Click its icon or press Ctrl+Alt+M to open it." => "انقر أيقونته أو اضغط Ctrl+Alt+M لفتحه.",
        "Click its icon to open it." => "انقر أيقونته لفتحه.",
        "Reset position" => "إعادة ضبط المكان",
        // Dialogs
        "Switch account?" => "تبديل الحساب؟",
        "Confirm switch" => "تأكيد التبديل",
        "Switch now" => "بدّل الآن",
        "Confirm to open this saved account. If Claude is running, it will close first. Local tasks may stop and unsaved work may be lost." => "أكّد لفتح هذا الحساب المحفوظ. لو Claude شغال هيتقفل الأول. المهام المحلية ممكن تتوقف والشغل غير المحفوظ ممكن يضيع.",
        "Switch the Codex file sign-in? New CLI sessions use this account. Restart open sessions to reload. The Desktop host may keep its own login." => "تبديل تسجيل دخول Codex؟ جلسات CLI الجديدة ستستخدم هذا الحساب. أعد تشغيل الجلسات المفتوحة. تطبيق سطح المكتب قد يحتفظ بتسجيل دخوله.",
        "Switch the Codex sign-in cache? New Codex CLI sessions use this account. Restart existing sessions to reload it; the Desktop app may retain its own host login." => "تبديل تسجيل دخول Codex؟ جلسات Codex CLI الجديدة ستستخدم هذا الحساب. أعد تشغيل الجلسات الحالية؛ تطبيق سطح المكتب قد يحتفظ بتسجيل دخوله.",
        "Add another account?" => "إضافة حساب آخر؟",
        "Restore previous sign-in?" => "استعادة تسجيل الدخول السابق؟",
        "Close Claude & save?" => "إغلاق Claude والحفظ؟",
        "Close Claude & switch?" => "إغلاق Claude والتبديل؟",
        "Claude's current account is already saved. Claude will close and reopen at its sign-in screen so you can add another one. The saved account is not signed out." => "الحساب الحالي في Claude محفوظ بالفعل. Claude هيتقفل ويفتح على شاشة تسجيل الدخول علشان تضيف حساب تاني. الحساب المحفوظ مش هيتعمله تسجيل خروج.",
        "Claude Desktop will close. Local tasks may stop and unsaved work may be lost. Nothing closes until you confirm." => "Claude Desktop هيتقفل. المهام المحلية ممكن تتوقف والشغل غير المحفوظ ممكن يضيع. مفيش حاجة هتتقفل قبل ما تأكد.",
        "Close & sign in" => "إغلاق وتسجيل الدخول",
        "Close & restore" => "إغلاق واستعادة",
        "Close & save" => "إغلاق وحفظ",
        "Close & switch" => "إغلاق وتبديل",
        "Remove this account?" => "حذف هذا الحساب؟",
        "Remove account" => "حذف الحساب",
        "Remove {name} from Muisland? The current sign-in, chats and files remain unchanged. The saved sign-in is moved to an encrypted local archive." => "حذف {name} من Muisland؟ تسجيل الدخول الحالي والمحادثات والملفات لن تتغير. تسجيل الدخول المحفوظ ينتقل إلى أرشيف محلي مشفر.",
        "Usage limit reached" => "تم الوصول لحد الاستخدام",
        "Switch to {name}? Claude may close and running projects or unsaved work may stop. Muisland cannot reliably tell whether a task is idle. Codex sessions need a restart to use the new sign-in." => "التبديل إلى {name}؟ قد يُغلق Claude وقد تتوقف المشاريع الجارية أو الشغل غير المحفوظ. لا يستطيع Muisland معرفة إن كانت المهمة خاملة بدقة. جلسات Codex تحتاج إعادة تشغيل لاستخدام الحساب الجديد.",
        "Switching in {n} seconds" => "التبديل خلال {n} ثانية",
        "App may be working · confirmation required" => "التطبيق قد يكون يعمل · التأكيد مطلوب",
        // Status and notices
        "Already logged in" => "مسجّل الدخول بالفعل",
        "{name} is already logged into {app}. No sign-in was changed." => "{name} مسجّل الدخول بالفعل في {app}. لم يتغير شيء.",
        "Sign in to Claude" => "سجّل الدخول إلى Claude",
        "Claude opened. Complete sign-in there, then choose Save again. Your existing saved accounts were kept." => "تم فتح Claude. أكمل تسجيل الدخول هناك ثم اضغط حفظ مرة أخرى. حساباتك المحفوظة كما هي.",
        "Sign in to the other account" => "سجّل الدخول إلى الحساب الآخر",
        "{name} is already saved and was not signed out. Claude reopened at its sign-in screen: sign in to the other account there, then choose Save again. Avoid Claude's own Log out; it ends a saved session." => "{name} محفوظ بالفعل ولم يتم تسجيل خروجه. Claude فتح على شاشة تسجيل الدخول: سجّل الدخول بالحساب الآخر ثم اضغط حفظ مرة أخرى. تجنب زر تسجيل الخروج في Claude لأنه ينهي الجلسة المحفوظة.",
        "Couldn't finish this action" => "تعذّر إكمال العملية",
        "Usage alert" => "تنبيه الاستخدام",
        "{name} reached a configured usage threshold." => "{name} وصل لحد الاستخدام المحدد.",
        "No saved account has fresh, lower usage in both windows. No switch was made." => "لا يوجد حساب محفوظ باستخدام أقل وحديث في الفترتين. لم يتم التبديل.",
        "Open accounts to review usage or switch." => "افتح الحسابات لمراجعة الاستخدام أو التبديل.",
        "Auto-switch paused" => "تم إيقاف التبديل التلقائي",
        "Usage or account state changed. Review the accounts and switch manually." => "تغيّر الاستخدام أو حالة الحساب. راجع الحسابات وبدّل يدويًا.",
        "Account slots full" => "خانات الحسابات ممتلئة",
        "You can save up to {n} accounts on this device." => "يمكنك حفظ حتى {n} حسابًا على هذا الجهاز.",
        "Could not remove account" => "تعذّر حذف الحساب",
        "Preferences could not be saved. Your account entry has been kept." => "تعذّر حفظ الإعدادات. تم الإبقاء على الحساب.",
        "Account removed from Muisland" => "تم حذف الحساب من Muisland",
        "Account name saved" => "تم حفظ اسم الحساب",
        "Could not save the account name" => "تعذّر حفظ اسم الحساب",
        "Could not save your preferences" => "تعذّر حفظ إعداداتك",
        "Could not change the startup setting" => "تعذّر تغيير إعداد التشغيل مع ويندوز",
        "Use a name of 1–32 characters." => "استخدم اسمًا من ١ إلى ٣٢ حرفًا.",
        "Please wait for the current action" => "انتظر انتهاء العملية الحالية",
        "Wait for the current action" => "انتظر انتهاء العملية الحالية",
        "Wait for the current action before removing an account" => "انتظر انتهاء العملية الحالية قبل حذف حساب",
        "Wait for the current action before quitting" => "انتظر انتهاء العملية الحالية قبل الخروج",
        "Wait for the current operation before changing apps" => "انتظر انتهاء العملية الحالية قبل تغيير التطبيق",
        "Closing Claude, then continuing…" => "جاري إغلاق Claude ثم المتابعة…",
        "Checking your account…" => "جاري التحقق من حسابك…",
        "Signed-in account changed. Auto-switch cancelled." => "تغيّر الحساب المسجّل. تم إلغاء التبديل التلقائي.",
        "Preview complete · no account changes" => "انتهت المعاينة · لم يتغير أي حساب",
        "Codex switches only its sign-in cache. Chats and working folders are not moved or restored." => "Codex يبدّل تسجيل الدخول فقط. المحادثات ومجلدات العمل لا تُنقل ولا تُستعاد.",
        "Codex sign-in switched · restart existing Codex sessions to reload" => "تم تبديل دخول Codex · أعد تشغيل جلسات Codex الحالية",
        "Codex sign-in saved" => "تم حفظ دخول Codex",
        "Open Claude Desktop, sign in, then choose Save again." => "افتح Claude Desktop وسجّل الدخول ثم اضغط حفظ مرة أخرى.",
        "Claude is ready for another sign-in but could not reopen. Open Claude manually." => "Claude جاهز لتسجيل دخول آخر لكن تعذّر فتحه. افتح Claude يدويًا.",
        "Account saved · Claude reopened" => "تم حفظ الحساب · أعيد فتح Claude",
        "Account switched, but Claude could not reopen. Open Claude manually." => "تم تبديل الحساب لكن تعذّر فتح Claude. افتحه يدويًا.",
        "Switched successfully" => "تم التبديل بنجاح",
        "Previous sign-in restored, but Claude could not reopen. Open Claude manually." => "تمت استعادة تسجيل الدخول السابق لكن تعذّر فتح Claude. افتحه يدويًا.",
        "Previous sign-in restored · Claude reopened" => "تمت استعادة تسجيل الدخول السابق · أعيد فتح Claude",
        "Backup recovered to a separate folder" => "تم استرجاع النسخة الاحتياطية في مجلد منفصل",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn english_is_identity_and_arabic_falls_back() {
        assert_eq!(t(0, "Settings"), "Settings");
        assert_eq!(t(1, "Settings"), "الإعدادات");
        assert_eq!(t(1, "Something new"), "Something new");
    }
    #[test]
    fn placeholders_survive_translation() {
        for key in [
            "Resets in {t}",
            "{n} accounts",
            "Switching in {n} seconds",
            "{name} is already logged into {app}. No sign-in was changed.",
            "Remove {name} from Muisland? The current sign-in, chats and files remain unchanged. The saved sign-in is moved to an encrypted local archive.",
        ] {
            let ar = t(1, key);
            for p in ["{t}", "{n}", "{name}", "{app}"] {
                assert_eq!(key.contains(p), ar.contains(p), "{key}");
            }
        }
    }
}
