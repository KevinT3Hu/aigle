#pragma once
#include <QSettings>
#include <algorithm>
namespace aigle {
struct Preferences {
    bool recursive = false;
    bool hdr = false;
    int thumbnail = 280;
    int autosaveMs = 1200;
    bool autoDisplay = true;
    static Preferences load(QSettings& settings) {
        return {settings.value("preferences/recursive", false).toBool(),
                settings.value("preferences/hdr", false).toBool(),
                std::clamp(settings.value("preferences/thumbnail", 280).toInt(), 160, 340),
                std::clamp(settings.value("preferences/autosaveMs", 1200).toInt(), 500, 10000),
                settings.value("preferences/autoDisplay", true).toBool()};
    }
    bool save(QSettings& settings) const {
        settings.setValue("preferences/recursive", recursive);
        settings.setValue("preferences/hdr", hdr);
        settings.setValue("preferences/thumbnail", thumbnail);
        settings.setValue("preferences/autosaveMs", autosaveMs);
        settings.setValue("preferences/autoDisplay", autoDisplay);
        settings.sync();
        return settings.status() == QSettings::NoError;
    }
};
} // namespace aigle
