#include "commands.hpp"
#include "window.hpp"
#include "localization.hpp"
#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDoubleSpinBox>
#include <QFormLayout>
#include <QMessageBox>
#include <QPushButton>
#include <QSlider>
#include <QSignalBlocker>
#include <QVBoxLayout>
namespace aigle {
void Window::editPreferences() {
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Preferences"));
    dialog.setMinimumWidth(460);
    auto* layout = new QVBoxLayout(&dialog);
    auto* form = new QFormLayout;
    layout->addLayout(form);
    auto* language = new QComboBox;
    language->setObjectName("preferencesLanguage");
    language->addItem(tr("System language"), "system");
    language->addItem("English", "en");
    language->addItem(QString::fromUtf8("简体中文"), "zh-CN");
    language->setCurrentIndex(std::max(0, language->findData(languageChoice())));
    form->addRow(tr("Language"), language);
    auto* recursive = new QCheckBox(tr("Include subfolders when opening folders"));
    recursive->setChecked(preferences_.recursive);
    form->addRow(tr("Library"), recursive);
    auto* thumbnail = new QSpinBox;
    thumbnail->setRange(160, 340);
    thumbnail->setSuffix(tr(" px"));
    thumbnail->setValue(preferences_.thumbnail);
    form->addRow(tr("Preferred thumbnail width"), thumbnail);
    auto* hdr = new QCheckBox(tr("Request HDR canvas by default"));
    hdr->setChecked(preferences_.hdr);
    hdr->setToolTip(
        tr("Uses SDR when the current display cannot present HDR."));
    form->addRow(tr("Display"), hdr);
    auto* displayProfile = new QComboBox;
    displayProfile->addItems({tr("Auto · monitor ICC profile"), tr("sRGB")});
    displayProfile->setCurrentIndex(preferences_.autoDisplay ? 0 : 1);
    form->addRow(tr("SDR display profile"), displayProfile);
    auto* detected = new QLabel((!detectedDisplay_.name.isNull() ? detectedDisplay_.name : !detectedDisplay_.error.isNull() ? detectedDisplay_.error : tr("No monitor ICC profile detected; using sRGB")));
    detected->setWordWrap(true);
    form->addRow(tr("Detected monitor profile"), detected);
    auto* autosave = new QDoubleSpinBox;
    autosave->setRange(.5, 10);
    autosave->setDecimals(1);
    autosave->setSingleStep(.1);
    autosave->setSuffix(tr(" seconds"));
    autosave->setValue(preferences_.autosaveMs / 1000.0);
    form->addRow(tr("Autosave interval while editing"), autosave);
    auto* hint = new QLabel(tr("Save applies these settings immediately. Changing subfolder "
                               "inclusion refreshes the current folder. Autosave remains enabled; "
                               "opening another photo and closing the editor also save edits."));
    hint->setWordWrap(true);
    layout->addWidget(hint);
    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel |
                                         QDialogButtonBox::RestoreDefaults);
    layout->addWidget(buttons);
    bindTranslation(&dialog, [this, &dialog, language, recursive, thumbnail, hdr, displayProfile, detected, autosave, hint, form] {
        dialog.setWindowTitle(tr("Preferences"));
        const QSignalBlocker languageBlocker(language), profileBlocker(displayProfile);
        language->setItemText(0, tr("System language"));
        displayProfile->setItemText(0, tr("Auto · monitor ICC profile"));
        displayProfile->setItemText(1, tr("sRGB"));
        const auto label = [form](QWidget* control, const QString& text) {
            qobject_cast<QLabel*>(form->labelForField(control))->setText(text);
            control->setAccessibleName(text);
        };
        label(language, tr("Language"));
        label(recursive, tr("Library"));
        label(thumbnail, tr("Preferred thumbnail width"));
        label(hdr, tr("Display"));
        label(displayProfile, tr("SDR display profile"));
        label(detected, tr("Detected monitor profile"));
        label(autosave, tr("Autosave interval while editing"));
        recursive->setText(tr("Include subfolders when opening folders"));
        recursive->setAccessibleName(recursive->text());
        hdr->setText(tr("Request HDR canvas by default"));
        hdr->setAccessibleName(hdr->text());
        hdr->setToolTip(tr("Uses SDR when the current display cannot present HDR."));
        thumbnail->setSuffix(tr(" px"));
        autosave->setSuffix(tr(" seconds"));
        detected->setText((!detectedDisplay_.name.isNull() ? detectedDisplay_.name : !detectedDisplay_.error.isNull() ? detectedDisplay_.error : tr("No monitor ICC profile detected; using sRGB")));
        hint->setText(tr("Save applies these settings immediately. Changing subfolder "
                         "inclusion refreshes the current folder. Autosave remains enabled; "
                         "opening another photo and closing the editor also save edits."));
    });
    connect(buttons->button(QDialogButtonBox::RestoreDefaults), &QPushButton::clicked, &dialog,
            [&] {
                const Preferences defaults;
                language->setCurrentIndex(0);
                recursive->setChecked(defaults.recursive);
                hdr->setChecked(defaults.hdr);
                displayProfile->setCurrentIndex(0);
                thumbnail->setValue(defaults.thumbnail);
                autosave->setValue(defaults.autosaveMs / 1000.0);
            });
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, [&] {
        const Preferences next{recursive->isChecked(), hdr->isChecked(), thumbnail->value(),
                               qRound(autosave->value() * 1000), displayProfile->currentIndex() == 0};
        if (!next.save(settings_)) {
            QMessageBox::warning(&dialog, tr("Preferences"), tr("Could not save preferences."));
            return;
        }
        settings_.setValue("preferences/language", language->currentData());
        settings_.sync();
        if (settings_.status() != QSettings::NoError) {
            QMessageBox::warning(&dialog, tr("Preferences"), tr("Could not save preferences."));
            return;
        }
        const bool rescan = next.recursive != preferences_.recursive;
        preferences_ = next;
        applyDisplayProfile();
        autosave_.setInterval(next.autosaveMs);
        setHdr(next.hdr);
        findChild<QSlider*>("thumbnailSize")->setValue(next.thumbnail);
        if (rescan && !activeAlbumId_ && !folder_.isEmpty())
            emit request(commands::Scan{.path = folder_, .recursive = next.recursive, .preserve_mode = true});
        dialog.accept();
        applyTranslations(*qApp, language->currentData().toString());
    });
    dialog.exec();
}
} // namespace aigle
