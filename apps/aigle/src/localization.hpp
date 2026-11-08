#pragma once
#include <QCoreApplication>
#include <QLibraryInfo>
#include <QLocale>
#include <QSettings>
#include <QTranslator>
#include <QEvent>
#include <functional>
namespace aigle {
inline QString uiText(const QString& source) {
    return QCoreApplication::translate("aigle::Labels", source.toUtf8().constData());
}
inline QString languageChoice() {
    return QSettings().value("preferences/language", "system").toString();
}
inline void applyTranslations(QCoreApplication& app, const QString& choice) {
    const bool chinese = choice == "zh-CN" ||
        (choice == "system" && QLocale::system().language() == QLocale::Chinese);
    const auto effective = chinese ? QStringLiteral("zh-CN") : QStringLiteral("en");
    if (app.property("aigleEffectiveLanguage").toString() == effective) return;
    // Replacing two translators generates intermediate LanguageChange events.
    // Refresh our bindings once, after the complete language is installed.
    app.setProperty("aigleChangingLanguage", true);
    for (auto* translator : app.findChildren<QTranslator*>("aigleTranslator", Qt::FindDirectChildrenOnly)) {
        app.removeTranslator(translator);
        delete translator;
    }
    if (chinese) {
    auto* qt = new QTranslator(&app);
    qt->setObjectName("aigleTranslator");
    if (qt->load("qtbase_zh_CN", QLibraryInfo::path(QLibraryInfo::TranslationsPath)))
        app.installTranslator(qt);
    auto* editor = new QTranslator(&app);
    editor->setObjectName("aigleTranslator");
    if (editor->load(":/i18n/aigle_zh_CN.qm")) app.installTranslator(editor);
    }
    app.setProperty("aigleEffectiveLanguage", effective);
    app.setProperty("aigleChangingLanguage", false);
    QEvent changed(QEvent::LanguageChange);
    QCoreApplication::sendEvent(&app, &changed);
}

inline void installTranslations(QCoreApplication& app) {
    applyTranslations(app, languageChoice());
}
// Only explicitly bound application text is refreshed. Never discover strings by
// scanning widget contents: those contents can be filenames or unfinished edits.
class TranslationBinding final : public QObject {
    std::function<void()> refresh_;
public:
    TranslationBinding(QObject* owner, std::function<void()> refresh)
        : QObject(owner), refresh_(std::move(refresh)) {
        QCoreApplication::instance()->installEventFilter(this);
        refresh_();
    }
protected:
    bool eventFilter(QObject* watched, QEvent* event) override {
        if (watched == QCoreApplication::instance() && event->type() == QEvent::LanguageChange &&
            !watched->property("aigleChangingLanguage").toBool()) refresh_();
        return QObject::eventFilter(watched, event);
    }
};
inline void bindTranslation(QObject* owner, std::function<void()> refresh) {
    new TranslationBinding(owner, std::move(refresh));
}
} // namespace aigle
