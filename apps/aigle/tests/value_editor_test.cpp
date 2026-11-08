#include "editor_state_json.hpp"
#include "geometry_json.hpp"
#include "export_json.hpp"
#include "retouch_json.hpp"
#include "mask_json.hpp"
#include "agent_json.hpp"
#include "command_json.hpp"
#include "gui_json.hpp"
#include "adjustments.hpp"
#include "curve.hpp"
#include "scopes.hpp"
#include "localization.hpp"
#include "dialog_shortcuts.hpp"
#include "agent_panel.hpp"
#include "canvas.hpp"
#include "window.hpp"
#include "photo_identity.hpp"
#include "selection_metadata.hpp"
#include "export_dialog.hpp"
#include "retouch_panel.hpp"
#include "tool_panel.hpp"
#include "value_editor.hpp"
#include <QCheckBox>
#include <QAction>
#include <QBuffer>
#include <QComboBox>
#include <QDialog>
#include <QInputDialog>
#include <QFileDialog>
#include <QDialogButtonBox>
#include <QDoubleSpinBox>
#include <QJsonArray>
#include <QJsonDocument>
#include <QKeyEvent>
#include <QKeySequenceEdit>
#include <QTreeWidget>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QMouseEvent>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QScopeGuard>
#include <QSettings>
#include <QSignalSpy>
#include <QSlider>
#include <QTabBar>
#include <QTabWidget>
#include <QTemporaryDir>
#include <QTest>
#include <QTextBrowser>
#include <QThread>
#include <QTimer>
#include <QToolButton>
#include <QWheelEvent>
#include <QUuid>

static QJsonArray exportRequestsJson(const QList<aigle::commands::Export>& requests) {
    QJsonArray result;
    for (const auto& request : requests) result.append(aigle::toJson(request));
    return result;
}

class ValueEditorTest final : public QObject {
    Q_OBJECT
    QJsonObject retouchParams() const {
        return {{"spots", QJsonArray{QJsonObject{{"mode", "heal"},
                                                 {"target", QJsonObject{{"x", 0.4}, {"y", 0.4}}},
                                                 {"source", QJsonObject{{"x", 0.7}, {"y", 0.6}}},
                                                 {"radius", 0.1},
                                                 {"feather", 50},
                                                 {"opacity", 0.8}}}},
                {"red_eye", QJsonArray{QJsonObject{{"center", QJsonObject{{"x", 0.2}, {"y", 0.7}}},
                                                   {"radius", 0.02},
                                                   {"pupil", 40},
                                                   {"darken", 60}}}}};
    }
    QJsonObject retouchGuide() const {
        return {{"kind", "spot"},
                {"index", 0},
                {"value", retouchParams()["spots"].toArray()[0]},
                {"target", QJsonObject{{"x", 40}, {"y", 40}}},
                {"source", QJsonObject{{"x", 70}, {"y", 60}}},
                {"radius_handle", QJsonObject{{"x", 50}, {"y", 40}}},
                {"outline",
                 QJsonArray{QJsonObject{{"x", 50}, {"y", 40}}, QJsonObject{{"x", 40}, {"y", 50}},
                            QJsonObject{{"x", 30}, {"y", 40}}}}};
    }
  private slots:
    void initTestCase() { QMetaType::registerConverter<aigle::CurvePoints, QJsonArray>([](const aigle::CurvePoints& points) { return aigle::toJson(points); }); QMetaType::registerConverter<aigle::EngineCommand, QJsonObject>([](const aigle::EngineCommand& command) { return aigle::toJson(command); }); }
    void typedCommandQueuedDelivery() {
        aigle::Canvas canvas(nullptr);
        QObject receiver;
        std::optional<aigle::EngineCommand> received;
        QObject::connect(&canvas, &aigle::Canvas::toolCommand, &receiver,
            [&](const aigle::EngineCommand& command) { received = command; }, Qt::QueuedConnection);
        emit canvas.toolCommand(aigle::commands::Open{.path = QString("photo.raw"), .develop = false});
        QVERIFY(!received);
        QTRY_VERIFY(received.has_value());
        const auto* command = std::get_if<aigle::commands::Open>(&*received);
        QVERIFY(command);
        QCOMPARE(command->path.value(), QString("photo.raw"));
        QCOMPARE(command->develop.value(), false);
        QVERIFY(!command->copy);
    }
    void typedOverlayPreservesMissingAndClearedFields() {
        const auto omitted = aigle::overlayPatchFromJson({});
        QVERIFY(!omitted.mask);
        QVERIFY(!omitted.proof);
        QVERIFY(aigle::toJson(omitted).isEmpty());
        const QJsonObject wire{{"mask_overlay", QJsonValue::Null}, {"proof", QJsonValue::Null}, {"clipping", false}};
        const auto cleared = aigle::overlayPatchFromJson(wire);
        QVERIFY(cleared.mask.has_value());
        QVERIFY(!cleared.mask->has_value());
        QVERIFY(cleared.proof.has_value());
        QVERIFY(!cleared.proof->has_value());
        QCOMPARE(aigle::toJson(cleared), wire);
        auto patch = aigle::overlayPatchFromJson({{"mask_overlay", 17}, {"proof", "monitor.icc"}});
        patch.apply(omitted);
        QCOMPARE(patch.mask->value(), qint64(17));
        patch.apply(cleared);
        QCOMPARE(aigle::toJson(patch), wire);
    }
    void typedStatePreservesIdentityAndMetadataDelta() {
        const auto state = aigle::editorStateFromJson({{"open", true}, {"copy", 42},
            {"photo_identity", "photo.raw#42"}, {"document_id", 11}, {"revision", 9},
            {"iptc", QJsonObject{{"title", "Photo title"}}}});
        QCOMPARE(state.copy.value(), qint64(42));
        QCOMPARE(state.photoIdentity, QString("photo.raw#42"));
        QCOMPARE(state.document, quint64(11));
        QCOMPARE(state.revision, quint64(9));
        QCOMPARE(state.iptc->value("title"), QString("Photo title"));
        QVERIFY(!state.params);
        const auto empty = aigle::editorStateFromJson({{"params", QJsonObject{}}});
        QVERIFY(empty.params.has_value());
        QVERIFY(!empty.iptc);
    }
    void comparisonCanvasKeyboardNavigationAndCommands() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(600, 400);
        canvas.setExtent({3000, 2000});
        canvas.setSurveyMode(true);
        const auto old = qApp->property("aigleEffectiveShortcuts");
        const auto restore = qScopeGuard([&] { qApp->setProperty("aigleEffectiveShortcuts", old); });
        qApp->setProperty("aigleEffectiveShortcuts", QVariantMap{
            {"comparison_fit", "Ctrl+F"}, {"comparison_pixels", "Ctrl+1"}, {"dialog_cancel", "Ctrl+Q"}});
        QSignalSpy fit(&canvas, &aigle::Canvas::surveyFitRequested);
        QSignalSpy pixels(&canvas, &aigle::Canvas::surveyPixelsRequested);
        QSignalSpy close(&canvas, &aigle::Canvas::surveyCloseRequested);
        const auto press = [&](int key, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
            QKeyEvent event(QEvent::KeyPress, key, modifiers);
            QApplication::sendEvent(&canvas, &event);
        };
        press(Qt::Key_Plus);
        QVERIFY(canvas.navigation().zoom > 1);
        const auto before = canvas.navigation().center;
        press(Qt::Key_Right);
        QVERIFY(canvas.navigation().center.x() > before.x());
        press(Qt::Key_F); press(Qt::Key_1); press(Qt::Key_Escape);
        QCOMPARE(fit.size() + pixels.size() + close.size(), 0);
        press(Qt::Key_F, Qt::ControlModifier);
        press(Qt::Key_1, Qt::ControlModifier);
        press(Qt::Key_Q, Qt::ControlModifier);
        QCOMPARE(fit.size(), 1); QCOMPARE(pixels.size(), 1); QCOMPARE(close.size(), 1);
    }
    void preferencesApplyLanguageOnlyOnSave() {
        const auto previousOrganization = QCoreApplication::organizationName();
        QCoreApplication::setOrganizationName("AiglePreferenceTest");
        const auto restoreOrganization = qScopeGuard([&] {
            QCoreApplication::setOrganizationName(previousOrganization);
        });
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, dir.path());
        const auto previousData = qgetenv("XDG_DATA_HOME");
        qputenv("XDG_DATA_HOME", dir.path().toUtf8());
        const auto restore = qScopeGuard([&] {
            aigle::applyTranslations(*qApp, "en");
            if (previousData.isNull()) qunsetenv("XDG_DATA_HOME");
            else qputenv("XDG_DATA_HOME", previousData);
        });
        QSettings().setValue("preferences/language", "en");
        aigle::applyTranslations(*qApp, "en");
        aigle::Window window(nullptr, QString(), false);
        QAction* preferences = nullptr;
        QAction* exportAction = nullptr;
        for (auto* action : window.findChildren<QAction*>()) {
            if (action->property("commandId") == "preferences") preferences = action;
            if (action->property("commandId") == "export") exportAction = action;
        }
        QVERIFY(preferences && exportAction);
        const auto english = exportAction->text();
        const auto choose = [&](const QString& choice, QDialogButtonBox::StandardButton button) {
            bool visited = false;
            QTimer::singleShot(0, &window, [&] {
                auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
                QVERIFY(dialog);
                const auto closeOnFailure = qScopeGuard([&] { dialog->reject(); });
                auto* language = dialog->findChild<QComboBox*>("preferencesLanguage");
                QVERIFY(language);
                QCOMPARE(language->currentData().toString(), aigle::languageChoice());
                language->setCurrentIndex(language->findData(choice));
                dialog->findChild<QDialogButtonBox*>()->button(button)->click();
                visited = true;
            });
            preferences->trigger();
            QVERIFY(visited);
        };
        bool switchedWhileOpen = false;
        QTimer::singleShot(0, &window, [&] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto close = qScopeGuard([&] { dialog->reject(); });
            auto* language = dialog->findChild<QComboBox*>("preferencesLanguage");
            auto* thumbnail = dialog->findChild<QSpinBox*>();
            auto* autosave = dialog->findChild<QDoubleSpinBox*>();
            QVERIFY(language && thumbnail && autosave);
            language->setCurrentIndex(language->findData("zh-CN"));
            thumbnail->setValue(237);
            autosave->setValue(3.7);
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(dialog->windowTitle(), QCoreApplication::translate("aigle::Window", "Preferences"));
            QVERIFY(dialog->windowTitle() != QString("Preferences"));
            QCOMPARE(language->currentData().toString(), QString("zh-CN"));
            QCOMPARE(thumbnail->value(), 237);
            QCOMPARE(autosave->value(), 3.7);
            QVERIFY(autosave->suffix() != QString(" seconds"));
            QCOMPARE(aigle::languageChoice(), QString("en"));
            aigle::applyTranslations(*qApp, "en");
            QCOMPARE(dialog->windowTitle(), QString("Preferences"));
            QCOMPARE(autosave->suffix(), QString(" seconds"));
            QCOMPARE(autosave->value(), 3.7);
            switchedWhileOpen = true;
        });
        preferences->trigger();
        QVERIFY(switchedWhileOpen);
        choose("zh-CN", QDialogButtonBox::Cancel);
        QCOMPARE(aigle::languageChoice(), QString("en"));
        QCOMPARE(exportAction->text(), english);
        choose("zh-CN", QDialogButtonBox::Save);
        QCOMPARE(aigle::languageChoice(), QString("zh-CN"));
        QCOMPARE(qApp->property("aigleEffectiveLanguage").toString(), QString("zh-CN"));
        QVERIFY(exportAction->text() != english);
        choose("en", QDialogButtonBox::Cancel);
        QCOMPARE(aigle::languageChoice(), QString("zh-CN"));
        QVERIFY(exportAction->text() != english);
        choose("en", QDialogButtonBox::Save);
        QCOMPARE(aigle::languageChoice(), QString("en"));
        QCOMPARE(exportAction->text(), english);
        const auto action = [&](const QString& id) -> QAction* {
            for (auto* candidate : window.findChildren<QAction*>())
                if (candidate->property("commandId") == id) return candidate;
            return nullptr;
        };
        for (const auto& id : {"language_chinese", "language_english", "display_srgb", "display_auto", "reload_shortcuts"})
            QVERIFY(action(id));
        action("language_chinese")->trigger();
        QCOMPARE(aigle::languageChoice(), QString("zh-CN"));
        QVERIFY(exportAction->text() != english);
        action("language_english")->trigger();
        QCOMPARE(aigle::languageChoice(), QString("en"));
        QCOMPARE(exportAction->text(), english);
        action("display_srgb")->trigger();
        QCOMPARE(QSettings().value("preferences/autoDisplay").toBool(), false);
        action("display_auto")->trigger();
        QCOMPARE(QSettings().value("preferences/autoDisplay").toBool(), true);
        QSettings external;
        external.setValue("shortcuts/language_chinese", "Ctrl+Alt+L");
        external.sync();
        const auto count = window.findChildren<QAction*>().size();
        action("reload_shortcuts")->trigger();
        QCOMPARE(action("language_chinese")->shortcut(), QKeySequence("Ctrl+Alt+L"));
        QCOMPARE(qApp->property("aigleEffectiveShortcuts").toMap()["language_chinese"].toString(), QString("Ctrl+Alt+L"));
        action("reload_shortcuts")->trigger();
        QCOMPARE(window.findChildren<QAction*>().size(), count);
        QVERIFY(action("keyboard_shortcuts"));
        const auto originalOpen = action("open_photo")->shortcut();
        bool checkedShortcutDialog = false;
        QTimer::singleShot(0, &window, [&] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto close = qScopeGuard([&] {
                aigle::applyTranslations(*qApp, "en");
                dialog->reject();
            });
            auto* tree = dialog->findChild<QTreeWidget*>();
            QVERIFY(tree && tree->topLevelItemCount() > 1);
            auto* first = qobject_cast<QKeySequenceEdit*>(tree->itemWidget(tree->topLevelItem(0), 1));
            auto* second = qobject_cast<QKeySequenceEdit*>(tree->itemWidget(tree->topLevelItem(1), 1));
            QVERIFY(first && second);
            const QKeySequence staged("Ctrl+Alt+Shift+F12");
            first->setKeySequence(staged);
            second->setKeySequence(staged);
            auto* save = dialog->findChild<QDialogButtonBox*>()->button(QDialogButtonBox::Save);
            QVERIFY(!save->isEnabled());
            const auto englishTitle = tree->topLevelItem(0)->text(0);
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(dialog->windowTitle(), QCoreApplication::translate("aigle::Window", "Keyboard shortcuts"));
            QVERIFY(tree->topLevelItem(0)->text(0) != englishTitle);
            QCOMPARE(first->accessibleName(), QCoreApplication::translate("aigle::Window", "Shortcut for %1").arg(tree->topLevelItem(0)->text(0)));
            QCOMPARE(first->keySequence(), staged);
            QCOMPARE(second->keySequence(), staged);
            QVERIFY(!save->isEnabled());
            second->clear();
            QVERIFY(save->isEnabled());
            QCOMPARE(action("open_photo")->shortcut(), originalOpen);
            checkedShortcutDialog = true;
        });
        action("keyboard_shortcuts")->trigger();
        QVERIFY(checkedShortcutDialog);
        QCOMPARE(action("open_photo")->shortcut(), originalOpen);
        QVERIFY(action("import_photos"));
        QSettings().setValue("import/folders", "Original folders");
        const bool nativeDialogsDisabled = QApplication::testAttribute(Qt::AA_DontUseNativeDialogs);
        QApplication::setAttribute(Qt::AA_DontUseNativeDialogs, true);
        const auto restoreDialogs = qScopeGuard([&] {
            QApplication::setAttribute(Qt::AA_DontUseNativeDialogs, nativeDialogsDisabled);
        });
        QSignalSpy importCommands(&window, &aigle::Window::request);
        for (const bool accept : {false, true}) {
            int stage = 0;
            bool verified = false;
            QTimer driver;
            driver.setInterval(10);
            connect(&driver, &QTimer::timeout, &window, [&] {
                auto* modal = qobject_cast<QDialog*>(QApplication::activeModalWidget());
                if (!modal) return;
                if (auto* picker = qobject_cast<QFileDialog*>(modal)) {
                    picker->setDirectory(dir.path());
                    picker->selectFile(dir.path());
                    ++stage;
                    QMetaObject::invokeMethod(picker, "accept");
                    return;
                }
                driver.stop();
                const auto close = qScopeGuard([&] { if (modal->result() != QDialog::Accepted) modal->reject(); });
                QCOMPARE(stage, 2);
                auto* folders = modal->findChild<QLineEdit*>("importFolders");
                auto* names = modal->findChild<QLineEdit*>("importNames");
                auto* backup = modal->findChild<QLineEdit*>("importBackup");
                QVERIFY(folders && names && backup);
                QCOMPARE(folders->text(), QString("Original folders"));
                folders->setText("{year}/Draft");
                names->setText("{name}-draft");
                backup->setText("/draft/backup");
                aigle::applyTranslations(*qApp, "zh-CN");
                QVERIFY(modal->windowTitle() != QString("Import photos"));
                QCOMPARE(folders->text(), QString("{year}/Draft"));
                QCOMPARE(names->text(), QString("{name}-draft"));
                QCOMPARE(backup->text(), QString("/draft/backup"));
                QVERIFY(backup->placeholderText() != QString("Optional second destination"));
                aigle::applyTranslations(*qApp, "en");
                verified = true;
                if (accept) modal->accept();
            });
            driver.start();
            action("import_photos")->trigger();
            driver.stop();
            QVERIFY(verified);
            QCOMPARE(QSettings().value("import/folders").toString(),
                     accept ? QString("{year}/Draft") : QString("Original folders"));
        }
        int imports = 0;
        for (const auto& arguments : importCommands) {
            const auto command = arguments[0].toJsonObject();
            if (command["cmd"] != "import") continue;
            ++imports;
            QCOMPARE(command["folders"].toString(), QString("{year}/Draft"));
            QCOMPARE(command["names"].toString(), QString("{name}-draft"));
            QCOMPARE(command["backup"].toString(), QString("/draft/backup"));
        }
        QCOMPARE(imports, 1);
        const QVariantList searches{QVariantMap{{"name", "Library"},
            {"filter", QVariantMap{{"query", "Literal query"}, {"rating", "Picked"}}},
            {"sort", QVariantMap{{"key", "Rating"}, {"descending", true}}}}};
        QSettings().setValue("library/savedSearches", searches);
        QVERIFY(action("manage_saved_searches"));
        bool reviewedSearch = false;
        QTimer::singleShot(0, &window, [&] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto close = qScopeGuard([&] {
                aigle::applyTranslations(*qApp, "en");
                if (dialog->result() != QDialog::Accepted) dialog->reject();
            });
            auto* list = dialog->findChild<QListWidget*>("savedSearchList");
            QVERIFY(list);
            QCOMPARE(list->count(), 1);
            list->setCurrentRow(0);
            auto* selected = list->currentItem();
            aigle::applyTranslations(*qApp, "zh-CN");
            QVERIFY(dialog->windowTitle() != QString("Saved searches"));
            QCOMPARE(list->currentItem(), selected);
            QCOMPARE(selected->text(), QString("Library"));
            QCOMPARE(QSettings().value("library/savedSearches").toList(), searches);
            auto* open = dialog->findChild<QPushButton*>("openSavedSearch");
            QVERIFY(open && open->isEnabled());
            QVERIFY(open->text() != QString("Open"));
            reviewedSearch = true;
            open->click();
        });
        action("manage_saved_searches")->trigger();
        QVERIFY(reviewedSearch);
        QCOMPARE(window.findChild<QLineEdit*>("librarySearch")->text(), QString("Literal query"));
        QCOMPARE(window.findChild<QComboBox*>("librarySort")->currentIndex(), 1);
        QVERIFY(window.findChild<QToolButton*>("libraryDescending")->isChecked());
        QCOMPARE(QSettings().value("library/savedSearches").toList(), searches);
        auto* manage = action("manage_saved_searches");
        QVERIFY(manage);
        auto driveNameDialog = [&](const QString& name, const QString& expectedCurrent,
                                  bool confirmDuplicate) {
            bool nameReviewed = false;
            QTimer::singleShot(0, &window, [&] {
                auto* manager = qobject_cast<QDialog*>(QApplication::activeModalWidget());
                QVERIFY(manager);
                auto* list = manager->findChild<QListWidget*>("savedSearchList");
                auto* save = [&]() -> QPushButton* {
                    for (auto* button : manager->findChildren<QPushButton*>())
                        if (button->text() == aigle::Window::tr("Save current…")) return button;
                    return nullptr;
                }();
                QVERIFY(list && save);
                QVERIFY(list->count() > 0);
                list->setCurrentRow(0);
                QCOMPARE(list->currentItem()->text(), expectedCurrent);
                QTimer::singleShot(0, manager, [&] {
                    auto* nameDialog = qobject_cast<QInputDialog*>(QApplication::activeModalWidget());
                    QVERIFY(nameDialog);
                    QCOMPARE(nameDialog->objectName(), QString("savedSearchNameDialog"));
                    nameDialog->setTextValue(name);
                    aigle::applyTranslations(*qApp, "zh-CN");
                    QVERIFY(nameDialog->windowTitle() != QString("Saved search"));
                    QCOMPARE(nameDialog->textValue(), name);
                    aigle::applyTranslations(*qApp, "en");
                    nameDialog->accept();
                    if (confirmDuplicate) {
                        QTimer::singleShot(0, manager, [&] {
                            auto* message = qobject_cast<QMessageBox*>(QApplication::activeModalWidget());
                            QVERIFY(message);
                            message->accept();
                        });
                    }
                });
                save->click();
                nameReviewed = true;
                manager->reject();
            });
            manage->trigger();
            QVERIFY(nameReviewed);
        };
        driveNameDialog("Travel", "Library", false);
        auto saved = QSettings().value("library/savedSearches").toList();
        QCOMPARE(saved.size(), 2);
        QCOMPARE(saved.last().toMap().value("name").toString(), QString("Travel"));
        QCOMPARE(saved.last().toMap().value("filter").toMap().value("query").toString(), QString("Literal query"));
        QCOMPARE(saved.last().toMap().value("sort").toMap().value("key").toString(), QString("Rating"));
        QCOMPARE(saved.last().toMap().value("sort").toMap().value("descending").toBool(), true);
        driveNameDialog("Library", "Library", true);
        QCOMPARE(QSettings().value("library/savedSearches").toList(), saved);
        auto* filters = window.findChild<QToolButton*>("libraryRatingFilter");
        QVERIFY(filters);
        for (int round = 0; round < 2; ++round) {
            bool checked = false;
            QTimer::singleShot(0, &window, [&] {
                auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
                QVERIFY(dialog);
                const auto close = qScopeGuard([&] {
                    aigle::applyTranslations(*qApp, "en");
                    if (dialog->result() != QDialog::Accepted) dialog->reject();
                });
                auto* label = dialog->findChild<QComboBox*>("filterColorLabel");
                auto* camera = dialog->findChild<QComboBox*>("filter_camera");
                auto* rating = dialog->findChild<QComboBox*>("filterRating");
                QVERIFY(label && camera && rating);
                if (round == 0) {
                    label->setCurrentIndex(2);
                    label->setEditText("Custom label");
                    label->lineEdit()->setSelection(0, 6);
                    camera->setEditText("Literal camera");
                    rating->setCurrentIndex(7);
                    aigle::applyTranslations(*qApp, "zh-CN");
                    QVERIFY(dialog->windowTitle() != QString("Library filters"));
                    QCOMPARE(label->currentText(), QString("Custom label"));
                    QCOMPARE(label->lineEdit()->selectedText(), QString("Custom"));
                    QCOMPARE(camera->currentText(), QString("Literal camera"));
                    QCOMPARE(rating->currentIndex(), 7);
                    dialog->accept();
                } else {
                    QCOMPARE(label->currentText(), QString("Custom label"));
                    QCOMPARE(camera->currentText(), QString("Literal camera"));
                    QCOMPARE(rating->currentIndex(), 7);
                }
                checked = true;
            });
            filters->click();
            QVERIFY(checked);
        }




    }
    void providerDialogTranslationPreservesStagedFields() {
        aigle::AgentPanel panel;
        bool checked = false;
        QTimer::singleShot(0, [&] {
            const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            auto* name = dialog->findChild<QLineEdit*>("agentProviderName");
            auto* key = dialog->findChild<QLineEdit*>("agentApiKey");
            name->setText("Draft provider");
            key->setText("test-only-session-key");
            dialog->findChild<QLineEdit*>("agentEndpoint")->clear();
            dialog->findChild<QPushButton*>("agentDiscoverModels")->click();
            auto* status = dialog->findChild<QLabel*>("agentCredentialStatus");
            QCOMPARE(status->text(), QString("Enter a provider endpoint first."));
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(status->text(), aigle::AgentPanel::tr("Enter a provider endpoint first."));
            QVERIFY(status->text() != "Enter a provider endpoint first.");
            QCOMPARE(name->text(), QString("Draft provider"));
            QCOMPARE(key->text(), QString("test-only-session-key"));
            QCOMPARE(key->echoMode(), QLineEdit::Password);
            QCOMPARE(dialog->windowTitle(), aigle::AgentPanel::tr("Provider & model"));
            checked = true;
            dialog->reject();
        });
        panel.findChild<QToolButton*>()->click();
        QVERIFY(checked);
    }
    void scopeTranslationPreservesAnalysisChoices() {
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::Scopes scopes;
        auto* mode = scopes.findChild<QComboBox*>("scopeMode");
        auto* target = scopes.findChild<QComboBox*>("scopeGamutTarget");
        auto* logarithmic = scopes.findChild<QCheckBox*>("scopeLog");
        mode->setCurrentIndex(6);
        target->setCurrentIndex(2);
        logarithmic->setChecked(true);
        QSignalSpy modeChanges(mode, &QComboBox::currentIndexChanged);
        QSignalSpy commands(&scopes, &aigle::Scopes::command);
        scopes.histogramChanged(aigle::histogramFromJson(QJsonObject{{"ready", true}}));
        QTRY_COMPARE(commands.size(), 1);
        const auto request = commands.first()[0].toJsonObject();
        scopes.setResult(aigle::scopeResultFromJson({{"request_id", request["request_id"]}, {"available", true},
            {"width", 1}, {"height", 1}, {"thumbnail", QJsonArray{QJsonArray{0, 0, 0, 255}}},
            {"data", QJsonObject{{"count", 10}, {"low", 2}, {"high", 1}, {"gamut", 3}}}}));
        auto* statistics = scopes.findChild<QLabel*>("scopeStatistics");
        QVERIFY(statistics->text().contains("20.0"));
        commands.clear();
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(mode->currentText(), aigle::Scopes::tr("Vectorscope"));
        QVERIFY(mode->currentText() != "Vectorscope");
        QCOMPARE(mode->currentIndex(), 6);
        QCOMPARE(target->currentIndex(), 2);
        QVERIFY(logarithmic->isChecked());
        QCOMPARE(modeChanges.size(), 0);
        QCOMPARE(commands.size(), 0);
        QVERIFY(!statistics->text().contains("Black"));
        QVERIFY(statistics->text().contains("20.0"));
    }
    void agentTranslationPreservesDraftAndMovedPhotoContext() {
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::AgentPanel panel;
        panel.setPhoto("/old/Before.png", {}, "translation-test");
        auto* prompt = panel.findChild<QPlainTextEdit*>();
        QVERIFY(prompt);
        prompt->setPlainText("Keep my draft unchanged");
        panel.setPhoto("/new/After.png", {}, "translation-test");
        auto* transcript = panel.findChild<QTextBrowser*>("agentTranscript");
        const auto before = transcript->toHtml();
        panel.setState(aigle::agentStateFromJson({{"proposal", true}, {"params", QJsonObject{{"light", QJsonObject{{"exposure", 1.25}}}}},
            {"stats", QJsonObject{{"requests", 3}, {"seconds", 2.5}, {"input_tokens", 123}, {"output_tokens", 45}}}}), true);
        QSignalSpy languageCommands(&panel, &aigle::AgentPanel::command);
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(prompt->toPlainText(), QString("Keep my draft unchanged"));
        QCOMPARE(transcript->toHtml(), before);
        QVERIFY(prompt->placeholderText() != "Describe or refine this edit…");
        bool currentPath = false;
        for (auto* label : panel.findChildren<QLabel*>()) {
            QVERIFY(!label->text().contains("Before.png"));
            currentPath |= label->text().contains("After.png");
        }
        QVERIFY(currentPath);
        bool summaryTranslated = false, usageTranslated = false;
        for (auto* label : panel.findChildren<QLabel*>()) {
            summaryTranslated |= label->text().contains(aigle::uiText("Exposure")) && label->text().contains("1.25");
            usageTranslated |= label->text().contains("123") && !label->text().contains("requests");
        }
        QVERIFY(summaryTranslated);
        QVERIFY(usageTranslated);
        QCOMPARE(languageCommands.size(), 0);
        aigle::applyTranslations(*qApp, "en");
        QCOMPARE(prompt->placeholderText(), QString("Describe or refine this edit…"));
        QCOMPARE(prompt->toPlainText(), QString("Keep my draft unchanged"));
    }
    void profileTranslationsRetainStateAndWhiteBalanceSelection() {
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::applyTranslations(*qApp, "en");
        aigle::Adjustments editor;
        editor.setSchema({});
        editor.setParams({{"wb", QJsonObject{{"mode", "custom"}}},
            {"profile", QJsonObject{{"camera", QJsonObject{{"kind", "dcp"}}}, {"look", QJsonObject{{"path", "Look.cube"}}}}},
            {"lens", QJsonObject{{"profile", QJsonObject{{"kind", "resolved"}}}}}});
        QComboBox* mode = nullptr;
        for (auto* combo : editor.findChildren<QComboBox*>())
            if (combo->findData("as_shot") >= 0) mode = combo;
        QVERIFY(mode);
        QCOMPARE(mode->currentData().toString(), QString("custom"));
        const auto hasLabel = [&](const QString& text) {
            for (auto* label : editor.findChildren<QLabel*>()) if (label->text() == text) return true;
            return false;
        };
        QVERIFY(hasLabel("Imported DCP profile"));
        QVERIFY(hasLabel("Imported look LUT"));
        QVERIFY(hasLabel("Matched lens profile enabled"));
        auto* import = editor.findChild<QPushButton*>("import.dcp");
        QVERIFY(import);
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        aigle::applyTranslations(*qApp, "zh-CN");
        QVERIFY(import->text() != "Import camera profile…");
        QVERIFY(!hasLabel("Imported DCP profile"));
        QCOMPARE(mode->currentData().toString(), QString("custom"));
        aigle::applyTranslations(*qApp, "en");
        QVERIFY(hasLabel("Imported DCP profile"));
        QVERIFY(hasLabel("Imported look LUT"));
        QVERIFY(hasLabel("Matched lens profile enabled"));
        QCOMPARE(commands.size(), 0);
        // Replacing the schema destroys the old bindings along with their controls.
        editor.setSchema({});
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(commands.size(), 0);
    }
    void metadataTranslationKeepsMixedDraftAndAdjustmentSearch() {
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::applyTranslations(*qApp, "en");
        aigle::SelectionMetadata metadata;
        const QJsonArray targets{QJsonObject{{"path", "/a.png"}}, QJsonObject{{"path", "/b.png"}}};
        metadata.setSelection(aigle::photoIdentitiesFromJson(targets),aigle::selectionMetadataFromJson( QJsonArray{
            QJsonObject{{"meta_loaded", true}, {"iptc", QJsonObject{{"title", "One"}}}},
            QJsonObject{{"meta_loaded", true}, {"iptc", QJsonObject{{"title", "Two"}}}}}));
        auto* title = metadata.findChild<QLineEdit*>("metadata_title");
        QCOMPARE(title->placeholderText(), QString("Mixed values"));
        title->setText("Unfinished title");
        title->setModified(true);
        int writes = 0;
        metadata.command = [&](const aigle::EngineCommand&) { ++writes; };
        aigle::Adjustments adjustments;
        auto* search = adjustments.findChild<QLineEdit*>("adjustSearch");
        search->setText("exposure");
        adjustments.showPage(2);
        auto* tabs = adjustments.findChild<QTabWidget*>("developTabs");
        QSignalSpy commands(&adjustments, &aigle::Adjustments::command);
        aigle::applyTranslations(*qApp, "zh-CN");
        QVERIFY(title->accessibleName() != "Title");
        QVERIFY(title->placeholderText() != "Mixed values");
        QCOMPARE(title->text(), QString("Unfinished title"));
        QVERIFY(title->isModified());
        QCOMPARE(writes, 0);
        QCOMPARE(search->text(), QString("exposure"));
        QVERIFY(search->placeholderText() != "Find adjustment…");
        QCOMPARE(tabs->currentIndex(), 2);
        QCOMPARE(commands.size(), 0);
        aigle::applyTranslations(*qApp, "en");
        QCOMPARE(title->placeholderText(), QString("Mixed values"));
        QCOMPARE(title->accessibleName(), QString("Title"));
        QCOMPARE(tabs->tabText(2), QString("Masks"));
    }
    void translationBindingsPreserveUserContentAndReleaseDestroyedWidgets() {
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::applyTranslations(*qApp, "en");
        QLabel label;
        QLineEdit userText("Library");
        userText.setModified(true);
        aigle::bindTranslation(&label, [&] { label.setText(aigle::Window::tr("Library")); });
        int refreshCount = 0;
        QObject counterOwner;
        aigle::bindTranslation(&counterOwner, [&] { ++refreshCount; });
        QCOMPARE(label.text(), QString("Library"));
        for (int i = 0; i < 3; ++i) {
            const auto before = refreshCount;
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(refreshCount, before + 1);
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(refreshCount, before + 1);
            QTRY_COMPARE(label.text(), QString::fromUtf8("图库"));
            QCOMPARE(userText.text(), QString("Library"));
            QVERIFY(userText.isModified());
            aigle::applyTranslations(*qApp, "en");
            QTRY_COMPARE(label.text(), QString("Library"));
            QCOMPARE(qApp->findChildren<QTranslator*>("aigleTranslator").size(), 0);
        }
        int refreshed = 0;
        auto* owner = new QObject;
        aigle::bindTranslation(owner, [&] { ++refreshed; });
        delete owner;
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(refreshed, 1);
    }
    void duplicateReviewTranslationPreservesPathsAndSelection() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const auto oldData = qgetenv("XDG_DATA_HOME");
        qputenv("XDG_DATA_HOME", dir.path().toUtf8());
        const auto restore = qScopeGuard([&] {
            aigle::applyTranslations(*qApp, "en");
            if (oldData.isNull()) qunsetenv("XDG_DATA_HOME"); else qputenv("XDG_DATA_HOME", oldData);
        });
        aigle::applyTranslations(*qApp, "en");
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, dir.path());
        QImage image(32, 24, QImage::Format_RGB32);
        image.fill(Qt::gray);
        const auto first = dir.filePath("Library.png"), second = dir.filePath("Copy.png");
        QVERIFY(image.save(first));
        QVERIFY(QFile::copy(first, second));
        aigle::Window window(nullptr, first, false);
        auto* files = window.findChild<QListWidget*>("photos");
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 2, 10000);
        QAction* scan = nullptr;
        for (auto* action : window.findChildren<QAction*>())
            if (action->property("commandId") == "find_exact_duplicates") scan = action;
        QVERIFY(scan);
        bool reviewed = false;
        QTimer driver;
        driver.setInterval(10);
        connect(&driver, &QTimer::timeout, &window, [&] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            if (!dialog || dialog->objectName() != "duplicateReview") return;
            driver.stop();
            const auto close = qScopeGuard([&] { if (dialog->result() != QDialog::Accepted) dialog->reject(); });
            auto* tree = dialog->findChild<QTreeWidget*>();
            QVERIFY(tree);
            QCOMPARE(tree->topLevelItemCount(), 1);
            auto* group = tree->topLevelItem(0);
            QCOMPARE(group->childCount(), 2);
            const auto identity = group->data(0, Qt::UserRole);
            const auto filename = group->child(0)->text(0);
            tree->setCurrentItem(group->child(0));
            aigle::applyTranslations(*qApp, "zh-CN");
            QVERIFY(dialog->windowTitle() != QString("Exact duplicates"));
            QVERIFY(group->text(0) != QString("Group 1 — 2 photos"));
            QCOMPARE(group->data(0, Qt::UserRole), identity);
            QCOMPARE(group->child(0)->text(0), filename);
            QCOMPARE(tree->currentItem(), group->child(0));
            aigle::applyTranslations(*qApp, "en");
            QCOMPARE(group->text(0), QString("Group 1 — 2 photos"));
            reviewed = true;
            dialog->accept();
        });
        driver.start();
        scan->trigger();
        QTRY_VERIFY_WITH_TIMEOUT(reviewed, 10000);
        QCOMPARE(files->selectedItems().size(), 2);
        QVERIFY(QFileInfo::exists(first) && QFileInfo::exists(second));
    }
    void mixedSelectionMovePreservesOpenCopyAndDeduplicatesOriginals() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const auto previousData = qgetenv("XDG_DATA_HOME");
        qputenv("XDG_DATA_HOME", dir.path().toUtf8());
        const auto restore = qScopeGuard([&] {
            if (previousData.isNull()) qunsetenv("XDG_DATA_HOME");
            else qputenv("XDG_DATA_HOME", previousData);
        });
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, dir.path());
        QVERIFY(QDir(dir.path()).mkdir("source"));
        QVERIFY(QDir(dir.path()).mkdir("destination"));
        const auto source = dir.filePath("source/original.png");
        const auto destination = dir.filePath("destination");
        const auto moved = destination + "/original.png";
        QImage image(64, 48, QImage::Format_RGB32);
        image.fill(Qt::gray);
        QVERIFY(image.save(source));
        aigle::Window window(nullptr, source, false);
        auto* files = window.findChild<QListWidget*>("photos");
        const auto action = [&](const QString& id) -> QAction* {
            for (auto* item : window.findChildren<QAction*>())
                if (item->property("commandId") == id) return item;
            return nullptr;
        };
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 1, 10000);
        window.request(aigle::commandFromJson({{"cmd", "create_copy"}, {"name", "Move copy"}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 2, 10000);
        QTRY_VERIFY_WITH_TIMEOUT(files->currentItem()->data(aigle::PhotoDelegate::CopyRole).toLongLong(), 10000);
        const auto copyId = files->currentItem()->data(aigle::PhotoDelegate::CopyRole).toLongLong();
        window.request(aigle::commandFromJson({{"cmd", "edit"}, {"label", "Unsaved copy edit"},
            {"ops", QJsonArray{QJsonObject{{"op", "set"}, {"path", "light.exposure"}, {"value", 1.25}}}}}).value());
        action("library")->trigger();
        for (int i = 0; i < files->count(); ++i) files->item(i)->setSelected(true);
        bool verifiedSync = false;
        QTimer::singleShot(0, &window, [&] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            const auto close = qScopeGuard([&] {
                aigle::applyTranslations(*qApp, "en");
                dialog->reject();
            });
            auto checks = dialog->findChildren<QCheckBox*>();
            QCOMPARE(checks.size(), 17);
            for (auto* check : checks) check->setChecked(check->property("group") == "geometry");
            aigle::applyTranslations(*qApp, "zh-CN");
            QVERIFY(dialog->windowTitle() != QString("Sync selected adjustments"));
            for (auto* check : checks) {
                QCOMPARE(check->isChecked(), check->property("group") == "geometry");
                if (check->property("group") == "geometry")
                    QVERIFY(check->text() != QString("Crop and geometry"));
            }
            aigle::applyTranslations(*qApp, "en");
            bool reviewed = false;
            QTimer::singleShot(0, dialog, [&] {
                auto* review = qobject_cast<QMessageBox*>(QApplication::activeModalWidget());
                QVERIFY(review);
                const auto dismiss = qScopeGuard([&] { review->accept(); });
                QVERIFY(review->text().contains(source));
                QVERIFY(review->text().contains("original.png"));
                reviewed = true;
            });
            auto* review = dialog->findChild<QPushButton*>("syncReviewTargets");
            QVERIFY(review);
            review->click();
            QVERIFY(reviewed);
            verifiedSync = true;
        });
        QVERIFY(action("sync_selected_adjustments_to_selection"));
        action("sync_selected_adjustments_to_selection")->trigger();
        QVERIFY(verifiedSync);
        QSignalSpy requests(&window, &aigle::Window::request);
        QTimer dialogs;
        dialogs.setInterval(10);
        int reviews = 0;
        connect(&dialogs, &QTimer::timeout, &window, [&] {
            if (auto* picker = qobject_cast<QFileDialog*>(QApplication::activeModalWidget())) {
                picker->selectFile(destination);
                QMetaObject::invokeMethod(picker, "accept");
            } else if (auto* review = qobject_cast<QMessageBox*>(QApplication::activeModalWidget())) {
                if (review->windowTitle() == "Review file changes") {
                    ++reviews;
                    review->button(QMessageBox::Ok)->click();
                }
            }
        });
        dialogs.start();
        action("move_selected_photos")->trigger();
        QTRY_VERIFY_WITH_TIMEOUT(QFileInfo::exists(moved) && !QFileInfo::exists(source), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 0, 10000);
        dialogs.stop();
        QCOMPARE(reviews, 1);
        int moves = 0;
        for (const auto& arguments : requests)
            if (arguments[0].toJsonObject()["cmd"] == "file_action") ++moves;
        QCOMPARE(moves, 1);
        window.request(aigle::commandFromJson({{"cmd", "scan"}, {"path", destination}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 2, 10000);
        bool foundCopy = false;
        for (int i = 0; i < files->count(); ++i) {
            QCOMPARE(files->item(i)->data(Qt::UserRole).toString(), moved);
            foundCopy |= files->item(i)->data(aigle::PhotoDelegate::CopyRole).toLongLong() == copyId;
        }
        QVERIFY(foundCopy);
        QTRY_VERIFY_WITH_TIMEOUT(files->currentItem() && files->currentItem()->data(aigle::PhotoDelegate::CopyRole).toLongLong() == copyId, 10000);
        // Inspect the reopened copy's export snapshot to verify unsaved edits
        // survived the move, rather than merely checking its label.
        bool preservedEdits = false;
        QTimer exportDialog;
        exportDialog.setInterval(10);
        connect(&exportDialog, &QTimer::timeout, &window, [&] {
            if (auto* dialog = qobject_cast<aigle::ExportDialog*>(QApplication::activeModalWidget())) {
                const auto inputs = exportRequestsJson(dialog->requests());
                preservedEdits = !inputs.isEmpty() && inputs[0].toObject()["params"].toObject()["light"].toObject()["exposure"].toDouble() == 1.25;
                exportDialog.stop();
                dialog->reject();
            }
        });
        exportDialog.start();
        action("export")->trigger();
        QTRY_VERIFY_WITH_TIMEOUT(preservedEdits, 10000);
        const auto retained = destination + "/retained.png";
        QVERIFY(image.save(retained));
        window.request(aigle::commandFromJson({{"cmd", "scan"}, {"path", destination}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 3, 10000);
        window.request(aigle::commandFromJson({{"cmd", "create_copies"}, {"name", "Disposable copy"},
            {"targets", QJsonArray{QJsonObject{{"path", retained}}}}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 4, 10000);
        files->clearSelection();
        for (int i = 0; i < files->count(); ++i) {
            auto* item = files->item(i);
            item->setSelected(item->data(Qt::UserRole).toString() == moved || item->data(aigle::PhotoDelegate::CopyRole).toLongLong());
        }
        requests.clear();
        bool correctCounts = false;
        QTimer::singleShot(0, [&] {
            if (auto* dialog = qobject_cast<QMessageBox*>(QApplication::activeModalWidget())) {
                correctCounts = dialog->text().contains("1 original photos") && dialog->text().contains("1 selected virtual copies");
                dialog->button(QMessageBox::Yes)->click();
            }
        });
        action("move_selected_photos_to_trash")->trigger();
        QVERIFY(correctCounts);
        QTRY_VERIFY_WITH_TIMEOUT(!QFileInfo::exists(moved), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 0, 10000); // source-folder refresh
        QVERIFY(QFileInfo::exists(retained));
        int physicalTrash = 0, copyDeletes = 0;
        for (const auto& arguments : requests) {
            const auto command = arguments[0].toJsonObject();
            physicalTrash += command["cmd"] == "file_action" && command["action"] == "trash";
            copyDeletes += command["cmd"] == "file_action" && command["action"] == "remove_copy";
        }
        QCOMPARE(physicalTrash, 1);
        QCOMPARE(copyDeletes, 1);
        window.request(aigle::commandFromJson({{"cmd", "scan"}, {"path", destination}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 1, 10000);
        QCOMPARE(files->item(0)->data(Qt::UserRole).toString(), retained);
    }
    void libraryCopiesKeepSeparateRowsAndMetadata() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, dir.path());
        const auto previousData = qgetenv("XDG_DATA_HOME");
        qputenv("XDG_DATA_HOME", dir.path().toUtf8());
        const auto restore = qScopeGuard([&] {
            if (previousData.isNull()) qunsetenv("XDG_DATA_HOME");
            else qputenv("XDG_DATA_HOME", previousData);
        });
        QImage image(64, 48, QImage::Format_RGB32);
        image.fill(Qt::gray);
        const auto path = dir.filePath("original.png");
        QVERIFY(image.save(path));
        QSettings().setValue("shortcuts/actual_pixels", "1");
        QSettings().setValue("shortcuts/export", "Ctrl+K, Ctrl+E");
        aigle::Window window(nullptr, path, false);
        const auto action = [&](const QString& id) -> QAction* {
            for (auto* candidate : window.findChildren<QAction*>())
                if (candidate->property("commandId").toString() == id) return candidate;
            return nullptr;
        };
        QVERIFY(action("rate_1") && action("rate_5") && action("actual_pixels") && action("export"));
        QCOMPARE(action("actual_pixels")->shortcut(), QKeySequence("1"));
        QVERIFY(action("rate_1")->shortcut().isEmpty());
        QCOMPARE(action("export")->shortcut(), QKeySequence("Ctrl+K, Ctrl+E"));
        QCOMPARE(action("rate_5")->shortcut(), QKeySequence("5"));
        auto* files = window.findChild<QListWidget*>("photos");
        QVERIFY(files);
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 1, 10000);
        auto* master = files->item(0);
        QTRY_VERIFY_WITH_TIMEOUT(master->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["meta_loaded"].toBool(), 10000);
        // Updating application labels must preserve live widget/document state.
        auto* draft = window.findChild<QLineEdit*>("metadata_title");
        QVERIFY(draft);
        const auto beforeText = draft->text();
        draft->setText("Library");
        draft->setModified(true);
        const auto beforeIdentity = aigle::photoIdentity(master);
        const auto beforeSelection = files->selectedItems();
        const auto exportBinding = action("export")->shortcut();
        QSignalSpy translationRequests(&window, &aigle::Window::request);
        QTabWidget* referenceTabs = nullptr;
        for (auto* tabs : window.findChildren<QTabWidget*>())
            if (tabs->count() == 3 && tabs->tabText(0) == "History") referenceTabs = tabs;
        QVERIFY(referenceTabs);
        referenceTabs->setCurrentIndex(1);
        {
            const auto restoreLanguage = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(action("library")->text(), QString::fromUtf8("图库"));
            QCOMPARE(draft->text(), QString("Library"));
            QVERIFY(draft->isModified());
            QCOMPARE(aigle::photoIdentity(master), beforeIdentity);
            QCOMPARE(files->selectedItems(), beforeSelection);
            QCOMPARE(action("export")->shortcut(), exportBinding);
            QCOMPARE(translationRequests.size(), 0);
            bool translatedCount = false;
            for (auto* label : window.findChildren<QLabel*>())
                translatedCount |= label->text() == aigle::Window::tr("%1 photos selected").arg(beforeSelection.size());
            QVERIFY(translatedCount);
            QCOMPARE(referenceTabs->currentIndex(), 1);
            QCOMPARE(referenceTabs->tabText(1), aigle::Window::tr("Snapshots"));
        }
        QCOMPARE(action("library")->text(), QString("Library"));
        draft->setText(beforeText);
        draft->setModified(false);
        window.request(aigle::commandFromJson({{"cmd", "create_copy"}, {"name", "Alternative"}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 2, 10000);
        QListWidgetItem* copy = nullptr;
        for (int i = 0; i < files->count(); ++i)
            if (files->item(i)->data(aigle::PhotoDelegate::CopyRole).toLongLong()) copy = files->item(i);
        QVERIFY(copy);
        QTRY_COMPARE_WITH_TIMEOUT(files->currentItem(), copy, 10000);
        QSignalSpy navigationRequests(&window, &aigle::Window::request);
        QVERIFY(action("navigate_left") && action("navigate_right"));
        action(files->row(master) < files->row(copy) ? "navigate_left" : "navigate_right")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(files->currentItem(), master, 10000);
        action(files->row(master) < files->row(copy) ? "navigate_right" : "navigate_left")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(files->currentItem(), copy, 10000);
        QJsonObject lastOpen;
        for (const auto& arguments : navigationRequests)
            if (arguments[0].toJsonObject()["cmd"] == "open") lastOpen = arguments[0].toJsonObject();
        QCOMPARE(lastOpen["copy"].toInteger(), copy->data(aigle::PhotoDelegate::CopyRole).toLongLong());
        const auto id = aigle::photoIdentity(copy);
        const auto masterRating = master->data(aigle::PhotoDelegate::RatingRole).toInt();
        action("rate_5")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(copy->data(aigle::PhotoDelegate::RatingRole).toInt(), 5, 10000);
        QCOMPARE(master->data(aigle::PhotoDelegate::RatingRole).toInt(), masterRating);
        QVERIFY(action("pick_photo") && action("label_blue") && action("clear_flag"));
        action("pick_photo")->trigger();
        action("label_blue")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(copy->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["flag"].toString(), QString("picked"), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(copy->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["label"].toString(), QString("blue"), 10000);
        action("clear_flag")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(copy->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["flag"].toString(), QString("none"), 10000);
        QCOMPARE(copy->data(aigle::PhotoDelegate::RatingRole).toInt(), 5);
        QCOMPARE(copy->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["label"].toString(), QString("blue"));
        QVERIFY(master->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["label"].isNull());
        auto* field = window.findChild<QLineEdit*>("metadata_title");
        QKeyEvent typing(QEvent::ShortcutOverride, Qt::Key_5, Qt::NoModifier);
        typing.ignore(); QApplication::sendEvent(field, &typing); QVERIFY(typing.isAccepted());
        auto* sort = window.findChild<QComboBox*>("librarySort");
        QKeyEvent navigation(QEvent::ShortcutOverride, Qt::Key_Down, Qt::NoModifier);
        navigation.ignore(); QApplication::sendEvent(sort, &navigation); QVERIFY(navigation.isAccepted());
        const auto original = aigle::photoIdentity(master);
        window.request(aigle::commandFromJson({{"cmd", "batch_metadata"}, {"targets", QJsonArray{aigle::toJson(original)}},
            {"tags", QJsonArray{"Shared", "Master only"}}, {"tag_mode", "replace"},
            {"iptc", QJsonObject{{"title", "Master title"}, {"caption", "Keep master caption"}}}}).value());
        window.request(aigle::commandFromJson({{"cmd", "batch_metadata"}, {"targets", QJsonArray{aigle::toJson(id)}},
            {"tags", QJsonArray{"Shared", "Copy only"}}, {"tag_mode", "replace"},
            {"iptc", QJsonObject{{"title", "Copy title"}, {"caption", "Keep copy caption"}}}}).value());
        master->setSelected(true);
        copy->setSelected(true);
        auto* title = window.findChild<QLineEdit*>("metadata_title");
        auto* keywords = window.findChild<QLineEdit*>("metadata_keywords");
        QVERIFY(title && keywords);
        QTRY_COMPARE_WITH_TIMEOUT(title->placeholderText(), QString("Mixed values"), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(keywords->text(), QString("Shared"), 10000);
        title->setText("Together"); title->setModified(true); title->editingFinished();
        keywords->setText("New shared"); keywords->setModified(true); keywords->editingFinished();
        const auto metadata = [](QListWidgetItem* row) { return row->data(aigle::PhotoDelegate::MetadataRole).toJsonObject(); };
        QTRY_COMPARE_WITH_TIMEOUT(metadata(master)["iptc"].toObject()["title"].toString(), QString("Together"), 10000);
        QTRY_COMPARE_WITH_TIMEOUT(metadata(copy)["iptc"].toObject()["title"].toString(), QString("Together"), 10000);
        QTRY_VERIFY_WITH_TIMEOUT(metadata(master)["tags"].toArray().contains("New shared"), 10000);
        QTRY_VERIFY_WITH_TIMEOUT(metadata(copy)["tags"].toArray().contains("New shared"), 10000);
        QVERIFY(metadata(master)["tags"].toArray().contains("Master only"));
        QVERIFY(metadata(copy)["tags"].toArray().contains("Copy only"));
        QVERIFY(!metadata(master)["tags"].toArray().contains("Shared"));
        QCOMPARE(metadata(master)["iptc"].toObject()["caption"].toString(), QString("Keep master caption"));
        QCOMPARE(metadata(copy)["iptc"].toObject()["caption"].toString(), QString("Keep copy caption"));
        // Export a real master/copy selection through the asynchronous dialog and queue.
        QSettings().setValue("export/destination", dir.path());
        QImage logo(16, 16, QImage::Format_ARGB32);
        logo.fill(Qt::red);
        const auto logoPath = dir.filePath("logo.png");
        QVERIFY(logo.save(logoPath));
        window.request(aigle::commandFromJson({{"cmd", "edit"}, {"label", "Copy exposure"},
            {"ops", QJsonArray{QJsonObject{{"op", "set"}, {"path", "light.exposure"}, {"value", 1.0}}}}}).value());
        QJsonArray exportJobs;
        bool textWatermark = false;
        QTimer exportDialogTimer;
        exportDialogTimer.setInterval(10);
        connect(&exportDialogTimer, &QTimer::timeout, &window, [&] {
            auto* dialog = qobject_cast<aigle::ExportDialog*>(QApplication::activeModalWidget());
            if (!dialog) return;
            exportDialogTimer.stop();
            auto* format = dialog->findChild<QComboBox*>("exportFormat");
            format->setCurrentIndex(format->findText("PNG"));
            dialog->findChild<QSpinBox*>("exportLongEdge")->setValue(64);
            dialog->findChild<QLineEdit*>("exportNaming")->setText(textWatermark ? "text-{seq}-{name}" : "batch-{seq}-{name}");
            dialog->findChild<QComboBox*>("exportWatermarkKind")->setCurrentIndex(textWatermark ? 1 : 2);
            dialog->findChild<QLineEdit*>("exportWatermarkText")->setText("Qt");
            dialog->findChild<QLineEdit*>("exportWatermarkImage")->setText(logoPath);
            dialog->findChild<QDoubleSpinBox*>("exportWatermarkSize")->setValue(25);
            dialog->findChild<QDoubleSpinBox*>("exportWatermarkOpacity")->setValue(100);
            dialog->findChild<QDoubleSpinBox*>("exportWatermarkMargin")->setValue(0);
            exportJobs = exportRequestsJson(dialog->requests());
            dialog->findChild<QPushButton*>("primary")->click();
        });
        exportDialogTimer.start();
        action("export")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(exportJobs.size(), 2, 10000);
        QString masterOutput, copyOutput;
        for (const auto& job : exportJobs) {
            const auto request = job.toObject();
            const auto output = request["options"].toObject()["path"].toString();
            if (request["source_name"].toString().contains("Alternative")) copyOutput = output;
            else masterOutput = output;
        }
        QVERIFY(!masterOutput.isEmpty() && !copyOutput.isEmpty() && masterOutput != copyOutput);
        QTRY_VERIFY_WITH_TIMEOUT(!QImage(masterOutput).isNull() && !QImage(copyOutput).isNull(), 15000);
        QVERIFY(QImage(masterOutput) != QImage(copyOutput));
        for (const auto& output : {masterOutput, copyOutput}) {
            const QImage rendered(output);
            const auto mark = rendered.pixelColor(rendered.width() - 3, rendered.height() - 3);
            QVERIFY2(mark.red() > 240 && mark.green() < 20 && mark.blue() < 20, qPrintable(mark.name()));
        }
        textWatermark = true;
        exportJobs = {};
        exportDialogTimer.start();
        action("export")->trigger();
        QTRY_COMPARE_WITH_TIMEOUT(exportJobs.size(), 2, 10000);
        QString textMaster;
        for (const auto& job : exportJobs) {
            const auto request = job.toObject();
            const auto options = request["options"].toObject();
            QCOMPARE(options["watermark"].toObject()["content"].toObject()["Text"].toString(), QString("Qt"));
            const auto output = options["path"].toString();
            QTRY_VERIFY_WITH_TIMEOUT(!QImage(output).isNull(), 15000);
            if (!request["source_name"].toString().contains("Alternative")) textMaster = output;
        }
        QVERIFY(!textMaster.isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(!QImage(textMaster).isNull(), 15000);
        const QImage textImage(textMaster);
        const auto background = textImage.pixelColor(2, 2);
        int ink = 0;
        for (int y = 0; y < textImage.height(); ++y)
            for (int x = 0; x < textImage.width(); ++x) {
                const auto pixel = textImage.pixelColor(x, y);
                if (pixel.red() > background.red() + 30 && pixel.green() > background.green() + 30) ++ink;
            }
        QVERIFY2(ink > 5, "Text watermark did not produce visible glyph pixels");
        QVERIFY(ink < textImage.width() * textImage.height() / 4);
        window.request(aigle::commandFromJson({{"cmd", "rename_copy"}, {"id", aigle::toJson(id)["copy"]}, {"name", "Renamed"}}).value());
        QTRY_VERIFY_WITH_TIMEOUT(copy->text().contains("Renamed"), 10000);
        QAction* trash = nullptr;
        for (auto* action : window.findChildren<QAction*>())
            if (action->property("commandId") == "move_selected_photos_to_trash") trash = action;
        QVERIFY(trash);
        files->clearSelection();
        copy->setSelected(true);
        bool protectedCopy = false;
        QTimer::singleShot(0, [&] {
            if (auto* message = qobject_cast<QMessageBox*>(QApplication::activeModalWidget())) {
                protectedCopy = message->text().contains("delete 1 selected virtual copies");
                message->reject();
            }
        });
        trash->trigger();
        QVERIFY(protectedCopy);
        QVERIFY(QFileInfo::exists(path));
        window.request(aigle::commandFromJson({{"cmd", "remove_copy"}, {"id", aigle::toJson(id)["copy"]}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 1, 10000);
        QCOMPARE(files->item(0), master);
        QVERIFY(QFileInfo::exists(path));
        // Library creation uses selected identities, including copies, without
        // opening a new document. Repeated names remain distinct per source.
        action("library")->trigger();
        master->setSelected(true);
        const auto createSelected = [&] {
            QTimer::singleShot(0, [&] {
                if (auto* dialog = qobject_cast<QInputDialog*>(QApplication::activeModalWidget())) {
                    dialog->setTextValue("Batch");
                    dialog->accept();
                }
            });
            action("create_virtual_copy")->trigger();
        };
        createSelected();
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 2, 10000);
        QTRY_COMPARE(files->selectedItems().size(), 1);
        auto* batch = files->selectedItems().first();
        QVERIFY(batch != master);
        const auto batchId = aigle::photoIdentity(batch);
        window.request(aigle::commandFromJson({{"cmd", "batch_metadata"}, {"targets", QJsonArray{aigle::toJson(batchId)}}, {"rating", 2}}).value());
        QTRY_COMPARE_WITH_TIMEOUT(batch->data(aigle::PhotoDelegate::RatingRole).toInt(), 2, 10000);
        master->setSelected(true);
        navigationRequests.clear();
        createSelected();
        QTRY_COMPARE_WITH_TIMEOUT(files->count(), 4, 10000);
        QTRY_COMPARE(files->selectedItems().size(), 2);
        QStringList names;
        QList<int> ratings;
        for (auto* item : files->selectedItems()) {
            QTRY_VERIFY_WITH_TIMEOUT(item->data(aigle::PhotoDelegate::MetadataRole).toJsonObject()["meta_loaded"].toBool(), 10000);
            names.append(item->text());
            ratings.append(item->data(aigle::PhotoDelegate::RatingRole).toInt());
        }
        QVERIFY(names[0] != names[1]);
        QVERIFY(ratings.contains(2));
        QVERIFY(ratings.contains(master->data(aigle::PhotoDelegate::RatingRole).toInt()));
        for (const auto& arguments : navigationRequests)
            QVERIFY(arguments[0].toJsonObject()["cmd"] != "open");
        files->clearSelection();
        batch->setSelected(true);
        QTimer::singleShot(0, [&] {
            if (auto* dialog = qobject_cast<QInputDialog*>(QApplication::activeModalWidget())) {
                dialog->setTextValue("Selected rename");
                dialog->accept();
            }
        });
        action("rename_selected_photo")->trigger();
        QTRY_VERIFY_WITH_TIMEOUT(([&] {
            for (int i = 0; i < files->count(); ++i)
                if (aigle::samePhoto(files->item(i), batchId) && files->item(i)->text().contains("Selected rename")) return true;
            return false;
        })(), 10000);
        window.request(aigle::commandFromJson({{"cmd", "open"}, {"path", batchId.path}, {"copy", batchId.copy}}).value());
        QTRY_VERIFY_WITH_TIMEOUT(files->currentItem() && aigle::samePhoto(files->currentItem(), batchId), 10000);
        action("library")->trigger();
        files->clearSelection();
        for (int i = 0; i < files->count(); ++i)
            if (files->item(i)->data(aigle::PhotoDelegate::CopyRole).toLongLong()) files->item(i)->setSelected(true);
        QString deletionError;
        QTimer deleteDialogs;
        deleteDialogs.setInterval(10);
        connect(&deleteDialogs, &QTimer::timeout, &window, [&] {
            if (auto* dialog = qobject_cast<QMessageBox*>(QApplication::activeModalWidget())) {
                if (auto* yes = dialog->button(QMessageBox::Yes)) yes->click();
                else { deletionError = dialog->text(); dialog->accept(); }
            }
        });
        deleteDialogs.start();
        trash->trigger();
        QTRY_VERIFY_WITH_TIMEOUT(([&] {
            for (int i = 0; i < files->count(); ++i)
                if (files->item(i)->data(aigle::PhotoDelegate::CopyRole).toLongLong()) return false;
            return true;
        })(), 10000);
        deleteDialogs.stop();
        QVERIFY2(deletionError.isEmpty(), qPrintable(deletionError));
        QVERIFY(QFileInfo::exists(path));
    }
    void canvasToolBindingsSupportChordsAndRemapping() {
        QTemporaryDir settingsDir;
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, settingsDir.path());
        QSettings settings;
        settings.setValue("shortcuts/tool_cancel", "Ctrl+K, Ctrl+C");
        settings.setValue("shortcuts/crop_accept", "Ctrl+K, Ctrl+A");
        settings.setValue("shortcuts/retouch_delete", "Ctrl+K, Ctrl+D");
        settings.setValue("shortcuts/retouch_delete_alternate", "");
        aigle::Canvas canvas(nullptr);
        canvas.resize(200, 100);
        canvas.setExtent({200, 100});
        canvas.setTool("crop");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        QSignalSpy finished(&canvas, &aigle::Canvas::toolFinished);
        const auto press = [&](int key, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
            QKeyEvent event(QEvent::KeyPress, key, modifiers);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        press(Qt::Key_Return);
        press(Qt::Key_Escape);
        QCOMPARE(commands.count(), 0);
        QCOMPARE(finished.count(), 0);
        press(Qt::Key_K, Qt::ControlModifier);
        QCOMPARE(commands.count(), 0);
        press(Qt::Key_A, Qt::ControlModifier);
        QCOMPARE(commands.count(), 1);
        QCOMPARE(commands.last()[0].toJsonObject()["tool"].toString(), QString("crop"));
        canvas.setTool("pan");
        press(Qt::Key_K, Qt::ControlModifier); press(Qt::Key_C, Qt::ControlModifier);
        QVERIFY(finished.count() >= 1);
        canvas.setRetouchState(42, 7,aigle::retouchGuidesFromJson( QJsonArray{retouchGuide()}));
        canvas.selectRetouch("spot", 0);
        canvas.setTool("retouch");
        commands.clear();
        press(Qt::Key_Delete); press(Qt::Key_Backspace);
        QCOMPARE(commands.count(), 0);
        press(Qt::Key_K, Qt::ControlModifier); press(Qt::Key_D, Qt::ControlModifier);
        QCOMPARE(commands.count(), 1);
        QCOMPARE(commands.last()[0].toJsonObject()["action"].toString(), QString("delete"));
        aigle::RetouchPanel panel;
        panel.setState(aigle::retouchParametersFromJson(retouchParams()), 42, 7,aigle::retouchGuidesFromJson( QJsonArray{retouchGuide()}));
        panel.select("spot", 0);
        auto* list = panel.findChild<QListWidget*>("retouchCorrections");
        QVERIFY(list);
        QSignalSpy panelCommands(&panel, &aigle::RetouchPanel::command);
        QTest::keyClick(list, Qt::Key_Delete);
        QCOMPARE(panelCommands.count(), 0);
        QTest::keyClick(list, Qt::Key_K, Qt::ControlModifier);
        QTest::keyClick(list, Qt::Key_D, Qt::ControlModifier);
        QCOMPARE(panelCommands.count(), 1);
        QCOMPARE(panelCommands.last()[0].toJsonObject()["action"].toString(), QString("delete"));
        // A prefix from one tool must not leak into the next tool.
        commands.clear();
        press(Qt::Key_K, Qt::ControlModifier);
        canvas.setTool("crop");
        press(Qt::Key_A, Qt::ControlModifier);
        QCOMPARE(commands.count(), 0);
    }
    void metadataCommitsToPreviousSelection() {
        aigle::SelectionMetadata panel;
        const QJsonObject first{{"path", "/first"}, {"copy", 3}};
        const QJsonObject second{{"path", "/second"}};
        const QJsonArray metadata{QJsonObject{{"meta_loaded", true}, {"iptc", QJsonObject{{"title", "Before"}}}}};
        QList<QJsonObject> commands;
        panel.command = [&](const aigle::EngineCommand& command) { commands.append(aigle::toJson(command)); };
        panel.setSelection(aigle::photoIdentitiesFromJson(QJsonArray{first}),aigle::selectionMetadataFromJson( metadata));
        auto* title = panel.findChild<QLineEdit*>("metadata_title");
        title->setText("Unfinished edit"); title->setModified(true);
        panel.setSelection(aigle::photoIdentitiesFromJson(QJsonArray{first}),aigle::selectionMetadataFromJson( metadata));
        QCOMPARE(title->text(), QString("Unfinished edit"));
        panel.setSelection(aigle::photoIdentitiesFromJson(QJsonArray{second}),aigle::selectionMetadataFromJson( metadata));
        QCOMPARE(commands.size(), 1);
        QCOMPARE(commands[0]["targets"].toArray(), QJsonArray{first});
        QCOMPARE(commands[0]["iptc"].toObject()["title"].toString(), QString("Unfinished edit"));
        QCOMPARE(title->text(), QString("Before"));
    }
    void composerBlocksApplicationShortcuts() {
        QWidget window;
        auto* panel = new aigle::AgentPanel(&window);
        auto* prompt = panel->findChild<QPlainTextEdit*>("agentPrompt");
        auto* provider = panel->findChild<QComboBox*>("agentProviderChoice");
        QVERIFY(prompt);
        QVERIFY(provider);
        for (QWidget* target : {static_cast<QWidget*>(prompt), static_cast<QWidget*>(provider)}) {
            QKeyEvent event(QEvent::ShortcutOverride, Qt::Key_Q, Qt::ControlModifier);
            event.ignore();
            QApplication::sendEvent(target, &event);
            QVERIFY(event.isAccepted());
        }
        prompt->setPlainText("hello");
        prompt->selectAll();
        QTest::keyClick(prompt, Qt::Key_C, Qt::ControlModifier);
        QTest::keyClick(prompt, Qt::Key_End);
        QTest::keyClick(prompt, Qt::Key_Return, Qt::ShiftModifier);
        QVERIFY(prompt->toPlainText().contains('\n'));
    }

    void nativeRetouchWorkflowUsesRealEngineAndCanvas() {
        if (!qEnvironmentVariableIsSet("AIGLE_NATIVE_RETOUCH_TEST"))
            QSKIP("Run explicitly with AIGLE_NATIVE_RETOUCH_TEST=1 on an XCB Vulkan surface");
        QVulkanInstance instance;
        QVERIFY2(instance.create(), "A working Vulkan presentation surface is required");
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        qRegisterMetaType<aigle::Frame>();
        QThread worker;
        auto* engine = new aigle::Engine;
        engine->moveToThread(&worker);
        QObject::connect(&worker, &QThread::finished, engine, &QObject::deleteLater);
        aigle::Canvas canvas(&instance);
        canvas.resize(480, 320);
        aigle::RetouchPanel panel;
        panel.resize(312, 700);
        auto cleanup = qScopeGuard([&] {
            if (worker.isRunning()) {
                QMetaObject::invokeMethod(engine, &aigle::Engine::stop,
                                          Qt::BlockingQueuedConnection);
                worker.quit();
                worker.wait();
            }
        });
        aigle::EditorState state;
        QString failure;
        const auto send = [&](QJsonObject request) {
            QMetaObject::invokeMethod(
                engine, [engine, request] { engine->command(aigle::commandFromJson(request).value()); }, Qt::QueuedConnection);
        };
        QObject::connect(engine, &aigle::Engine::error, &panel,
                         [&](const QString& message) { failure = message; });
        QObject::connect(&canvas, &aigle::Canvas::viewportChanged, &panel,
                         [&](const aigle::ViewportRequest& request) {
                             if (state.open)
                                 QMetaObject::invokeMethod(engine, [engine, request] { engine->setViewport(request); }, Qt::QueuedConnection);
                         });
        QObject::connect(&canvas, &aigle::Canvas::toolCommand, engine, &aigle::Engine::command,
                         Qt::QueuedConnection);
        QObject::connect(&panel, &aigle::RetouchPanel::command, engine, &aigle::Engine::command,
                         Qt::QueuedConnection);
        QObject::connect(&panel, &aigle::RetouchPanel::defaultsChanged, &canvas,
                         &aigle::Canvas::setRetouchDefaults);
        QObject::connect(&panel, &aigle::RetouchPanel::selected, &canvas,
                         &aigle::Canvas::selectRetouch);
        QObject::connect(&canvas, &aigle::Canvas::retouchSelected, &panel,
                         &aigle::RetouchPanel::select);
        QObject::connect(&canvas, &aigle::Canvas::retouchError, &panel,
                         [&](const QString& message) { failure = message; });
        QObject::connect(engine, &aigle::Engine::frameReady, &canvas, &aigle::Canvas::setFrame);
        QObject::connect(engine, &aigle::Engine::reply, &panel,
                         [&](const aigle::EngineReply& reply) {
                             if (reply.command.startsWith("retouch_")) {
                                 const auto& result = reply.get<aigle::RetouchResult>();
                                 canvas.setRetouchResult(result);
                                 panel.setResult(result);
                                 if (result.error) failure = *result.error;
                             }
                         });
        QObject::connect(
            engine, &aigle::Engine::stateChanged, &panel, [&](const aigle::EditorState& result) {
                const auto previous = state.params;
                state = result;
                if (result.extent) canvas.setExtent(*result.extent);
                if (result.params) {
                    canvas.setEditParams(result.geometry);
                    canvas.setRetouchState(result.document, result.revision, result.retouchGuides);
                    panel.setState(result.retouch, result.document, result.revision, result.retouchGuides);
                } else if (result.open) state.params = previous;
            });
        QSignalSpy ready(engine, &aigle::Engine::ready),
            rendered(&canvas, &aigle::Canvas::rendered),
            captured(&canvas, &aigle::Canvas::captured);
        worker.start();
        const aigle::EngineConfig config{.dataDirectory = storage.path(), .backend = aigle::EngineConfig::Backend::Cpu,
            .workerThreads = 2, .cacheDirectory = storage.path() + "/cache"};
        QMetaObject::invokeMethod(
            engine, [engine, config] { engine->start(config); }, Qt::QueuedConnection);
        QTRY_VERIFY_WITH_TIMEOUT(ready.count() > 0 || !failure.isEmpty(), 10000);
        QVERIFY2(failure.isEmpty(), qPrintable(failure));
        QCOMPARE(ready.count(), 1);
        panel.show();
        canvas.show();
        send({{"cmd", "synthetic"}});
        QTRY_VERIFY_WITH_TIMEOUT(state.load.status == aigle::ImageLoad::Status::Ready, 30000);
        QTRY_VERIFY_WITH_TIMEOUT(rendered.count() > 0, 30000);
        canvas.setTool("retouch");
        send({{"cmd", "tool_mode"}, {"tool", "retouch"}});
        panel.beginNew("clone");
        aigle::Viewport map;
        map.image = {1200, 800};
        const auto point = [&](QPointF crop) {
            map.output = QSizeF(canvas.size()) * canvas.devicePixelRatio();
            return (crop - map.region().topLeft()) * map.scale() / canvas.devicePixelRatio();
        };
        const auto mouse = [&](QEvent::Type type, QPointF position, Qt::MouseButton button,
                               Qt::MouseButtons buttons) {
            QMouseEvent event(type, position, position, button, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        const auto spots = [&] { return state.params.value_or(QJsonObject{})["spots"].toArray(); };
        mouse(QEvent::MouseButtonPress, point({360, 320}), Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, point({720, 320}), Qt::LeftButton, Qt::NoButton);
        QTRY_COMPARE_WITH_TIMEOUT(spots().size(), 1, 10000);
        QCOMPARE(spots()[0].toObject()["mode"].toString(), QString("clone"));
        auto* opacity = panel.findChild<QDoubleSpinBox*>("retouch.opacity");
        QVERIFY(opacity);
        QSignalSpy inspectorRequests(&panel, &aigle::RetouchPanel::command);
        opacity->setValue(65);
        QCOMPARE(inspectorRequests.count(), 1);
        QCOMPARE(inspectorRequests.last()[0].toJsonObject()["field"].toString(),
                 QString("opacity"));
        QTRY_VERIFY_WITH_TIMEOUT(!failure.isEmpty() ||
                                     std::abs(spots()[0].toObject()["opacity"].toDouble() - 0.65) <
                                         0.0001,
                                 10000);
        QVERIFY2(failure.isEmpty(), qPrintable(failure));
        send({{"cmd", "undo"}});
        QTRY_VERIFY(std::abs(spots()[0].toObject()["opacity"].toDouble() - 1) < 0.0001);
        send({{"cmd", "redo"}});
        QTRY_VERIFY(std::abs(spots()[0].toObject()["opacity"].toDouble() - 0.65) < 0.0001);
        map.output = QSizeF(canvas.size()) * canvas.devicePixelRatio();
        const QPointF anchor(canvas.width() / 2.0, canvas.height() / 2.0);
        QWheelEvent wheel(anchor, anchor, QPoint(), QPoint(0, 240), Qt::NoButton, Qt::NoModifier,
                          Qt::NoScrollPhase, false);
        QCoreApplication::sendEvent(&canvas, &wheel);
        map.zoom(map.scale() * std::pow(1.2, 2), anchor * canvas.devicePixelRatio());
        canvas.setTool("pan");
        mouse(QEvent::MouseButtonPress, anchor, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, anchor + QPointF(-25, 10), Qt::LeftButton, Qt::NoButton);
        map.pan(QPointF(-25, 10) * canvas.devicePixelRatio());
        canvas.setTool("retouch");
        const auto original = spots()[0].toObject();
        const auto revision = state.revision;
        const auto start = point({360, 320}), end = start + QPointF(20, 12);
        mouse(QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, end, Qt::NoButton, Qt::LeftButton);
        QTRY_VERIFY_WITH_TIMEOUT(state.retouchPreview, 10000);
        QCOMPARE(state.revision, revision);
        QCOMPARE(spots()[0].toObject(), original);
        bool pendingRendered = false;
        const auto pendingFrame = [&] {
            for (const auto& args : rendered)
                if (args[1].toULongLong() > quint64(revision))
                    return true;
            return false;
        };
        QTRY_VERIFY_WITH_TIMEOUT((pendingRendered = pendingFrame()), 30000);
        QVERIFY(pendingRendered);
        QKeyEvent escape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
        QCoreApplication::sendEvent(&canvas, &escape);
        QTRY_VERIFY_WITH_TIMEOUT(!state.retouchPreview, 10000);
        mouse(QEvent::MouseButtonRelease, end, Qt::LeftButton, Qt::NoButton);
        QTest::qWait(100);
        QCOMPARE(state.revision, revision);
        QCOMPARE(spots()[0].toObject(), original);
        mouse(QEvent::MouseButtonPress, start, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, end, Qt::LeftButton, Qt::NoButton);
        QTRY_VERIFY_WITH_TIMEOUT(spots()[0].toObject()["target"] != original["target"], 10000);
        QCOMPARE(spots()[0].toObject()["source"], original["source"]);
        send({{"cmd", "undo"}});
        QTRY_COMPARE_WITH_TIMEOUT(spots()[0].toObject(), original, 10000);
        const auto sourcePoint = point({720, 320});
        mouse(QEvent::MouseButtonPress, sourcePoint, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, sourcePoint + QPointF(-12, 8), Qt::LeftButton,
              Qt::NoButton);
        QTRY_VERIFY_WITH_TIMEOUT(spots()[0].toObject()["source"] != original["source"], 10000);
        QCOMPARE(spots()[0].toObject()["target"], original["target"]);
        send({{"cmd", "undo"}});
        QTRY_COMPARE_WITH_TIMEOUT(spots()[0].toObject(), original, 10000);
        const auto radiusPoint = point({378, 320});
        mouse(QEvent::MouseButtonPress, radiusPoint, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, radiusPoint + QPointF(10, 0), Qt::LeftButton,
              Qt::NoButton);
        QTRY_VERIFY_WITH_TIMEOUT(
            spots()[0].toObject()["radius"].toDouble() > original["radius"].toDouble(), 10000);
        QCOMPARE(spots()[0].toObject()["target"], original["target"]);
        send({{"cmd", "undo"}});
        QTRY_COMPARE_WITH_TIMEOUT(spots()[0].toObject(), original, 10000);
        QSignalSpy deleteRequests(&canvas, &aigle::Canvas::toolCommand);
        QKeyEvent deletion(QEvent::KeyPress, Qt::Key_Delete, Qt::NoModifier);
        QCoreApplication::sendEvent(&canvas, &deletion);
        QCOMPARE(deleteRequests.count(), 1);
        QCOMPARE(deleteRequests.last()[0].toJsonObject()["action"].toString(), QString("delete"));
        QVERIFY(deleteRequests.last()[0].toJsonObject()["expected"].isObject());
        QTRY_VERIFY_WITH_TIMEOUT(spots().isEmpty() || !failure.isEmpty(), 10000);
        QVERIFY2(failure.isEmpty(), qPrintable(failure));
        send({{"cmd", "undo"}});
        QTRY_COMPARE_WITH_TIMEOUT(spots().size(), 1, 10000);
        panel.beginNew("heal");
        mouse(QEvent::MouseButtonPress, point({1000, 600}), Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, point({1000, 600}), Qt::LeftButton, Qt::NoButton);
        QTRY_COMPARE_WITH_TIMEOUT(spots().size(), 2, 15000);
        QCOMPARE(spots()[1].toObject()["mode"].toString(), QString("heal"));
        panel.beginNew("red_eye");
        mouse(QEvent::MouseButtonPress, point({900, 500}), Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, point({900, 500}), Qt::LeftButton, Qt::NoButton);
        QTRY_COMPARE_WITH_TIMEOUT(state.params.value_or(QJsonObject{})["red_eye"].toArray().size(), 1, 10000);
        panel.findChild<QDoubleSpinBox*>("retouch.pupil")->setValue(62);
        QTRY_COMPARE_WITH_TIMEOUT(
            state.params.value_or(QJsonObject{})["red_eye"].toArray()[0].toObject()["pupil"].toInt(), 62,
            10000);
        canvas.fit();
        map.fit();
        canvas.capture(storage.path() + "/retouch-native.png");
        QTRY_VERIFY_WITH_TIMEOUT(captured.count() > 0, 10000);
        const auto image = captured.last()[0].value<QImage>();
        QVERIFY(!image.isNull());
        aigle::Viewport sample;
        sample.image = {1200, 800};
        sample.output = image.size();
        const auto left =
            ((QPointF(200, 100) - sample.region().topLeft()) * sample.scale()).toPoint();
        const auto right =
            ((QPointF(1000, 100) - sample.region().topLeft()) * sample.scale()).toPoint();
        QVERIFY(image.pixelColor(left) != image.pixelColor(right));
        QVERIFY2(failure.isEmpty(), qPrintable(failure));
    }
    void retouchInspectorEditsTypedFieldsAndCreationDefaults() {
        aigle::RetouchPanel panel;
        QSignalSpy commands(&panel, &aigle::RetouchPanel::command);
        panel.setState(aigle::retouchParametersFromJson(retouchParams()), 42, 7,aigle::retouchGuidesFromJson( {}));
        QCOMPARE(commands.count(), 0);
        auto* list = panel.findChild<QListWidget*>("retouchCorrections");
        QCOMPARE(list->count(), 2);
        auto* opacity = panel.findChild<QDoubleSpinBox*>("retouch.opacity");
        QCOMPARE(opacity->value(), 80.0);
        opacity->setValue(65);
        QCOMPARE(commands.count(), 1);
        auto request = commands.last().at(0).toJsonObject();
        QCOMPARE(request["cmd"].toString(), QString("retouch_commit"));
        QCOMPARE(request["kind"].toString(), QString("spot"));
        QCOMPARE(request["field"].toString(), QString("opacity"));
        QCOMPARE(request["value"].toDouble(), 0.65);
        QCOMPARE(request["document_id"].toInteger(), 42);
        QCOMPARE(request["revision"].toInteger(), 7);
        QCOMPARE(request["expected"].toObject()["source"],
                 retouchParams()["spots"].toArray()[0].toObject()["source"]);
        panel.select("eye", 0);
        auto* darken = panel.findChild<QDoubleSpinBox*>("retouch.darken");
        QCOMPARE(darken->value(), 60.0);
        darken->setValue(75);
        QCOMPARE(commands.last().at(0).toJsonObject()["kind"].toString(), QString("eye"));
        commands.clear();
        panel.beginNew("clone");
        panel.findChild<QDoubleSpinBox*>("retouch.radius")->setValue(3.5);
        QCOMPARE(commands.count(), 0);
        QCOMPARE(panel.defaults().tool, aigle::RetouchDefaults::Tool::Clone);
        QCOMPARE(panel.defaults().radius, 0.035);
        panel.select("spot", 0);
        panel.findChild<QPushButton*>("retouchDelete")->click();
        QCOMPARE(commands.last().at(0).toJsonObject()["action"].toString(), QString("delete"));
        const auto previous = commands.count();
        QTimer::singleShot(0, &panel, [&] {
            auto* box = panel.findChild<QMessageBox*>();
            QVERIFY(box);
            box->button(QMessageBox::Cancel)->click();
        });
        panel.findChild<QPushButton*>("retouchClear")->click();
        QCOMPARE(commands.count(), previous);
        QTimer::singleShot(0, &panel, [&] {
            auto* box = panel.findChild<QMessageBox*>();
            QVERIFY(box);
            box->button(QMessageBox::Yes)->click();
        });
        panel.findChild<QPushButton*>("retouchClear")->click();
        QCOMPARE(commands.last().at(0).toJsonObject()["action"].toString(), QString("clear"));
    }
    void retouchSliderPreviewCancelAndSingleCommit() {
        aigle::RetouchPanel panel;
        panel.setState(aigle::retouchParametersFromJson(retouchParams()), 42, 7,aigle::retouchGuidesFromJson( {}));
        panel.resize(312, 700);
        panel.show();
        QSignalSpy commands(&panel, &aigle::RetouchPanel::command);
        auto* number = panel.findChild<QDoubleSpinBox*>("retouch.radius");
        auto* slider = number->parentWidget()->findChild<QSlider*>();
        QVERIFY(slider);
        slider->setSliderDown(true);
        slider->setValue(100);
        QTRY_VERIFY(commands.count() >= 1);
        auto preview = commands.last().at(0).toJsonObject();
        QCOMPARE(preview["cmd"].toString(), QString("retouch_preview"));
        const auto beforeSwitch = commands.count();
        const auto stagedRadius = number->value();
        auto* corrections = panel.findChild<QListWidget*>("retouchCorrections");
        auto* selectedCorrection = corrections->currentItem();
        QVERIFY(selectedCorrection);
        const auto correctionIdentity = selectedCorrection->data(Qt::UserRole);
        {
            const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(number->value(), stagedRadius);
            QVERIFY(slider->isSliderDown());
            QCOMPARE(commands.count(), beforeSwitch);
            QCOMPARE(corrections->currentItem(), selectedCorrection);
            QCOMPARE(selectedCorrection->data(Qt::UserRole), correctionIdentity);
            QVERIFY(selectedCorrection->text().contains(aigle::RetouchPanel::tr("Heal")));
            QCOMPARE(number->accessibleName(), aigle::RetouchPanel::tr("Radius · % of source long edge"));
        }
        slider->setSliderDown(false);
        auto commit = commands.last().at(0).toJsonObject();
        QCOMPARE(commit["cmd"].toString(), QString("retouch_commit"));
        QCOMPARE(commit["session"], preview["session"]);
        commands.clear();
        slider->setSliderDown(true);
        slider->setValue(150);
        QTRY_VERIFY(commands.count() >= 1);
        QTest::keyClick(slider, Qt::Key_Escape);
        QCOMPARE(commands.last().at(0).toJsonObject()["cmd"].toString(), QString("retouch_cancel"));
        const auto count = commands.count();
        slider->setSliderDown(false);
        QTest::qWait(60);
        QCOMPARE(commands.count(), count);
        QCOMPARE(number->value(), 10.0);
    }
    void retouchCanvasSelectionPreviewCommitAndCancellation() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(100, 100);
        canvas.setExtent({100, 100});
        canvas.setRetouchState(42, 7,aigle::retouchGuidesFromJson( QJsonArray{retouchGuide()}));
        canvas.selectRetouch("spot", 0);
        canvas.setTool("retouch");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        QKeyEvent override(QEvent::ShortcutOverride, Qt::Key_Delete, Qt::NoModifier);
        override.ignore();
        QCoreApplication::sendEvent(&canvas, &override);
        QVERIFY(override.isAccepted());
        const auto mouse = [&](QEvent::Type type, QPointF at, Qt::MouseButton button,
                               Qt::MouseButtons buttons) {
            QMouseEvent event(type, at, at, button, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        mouse(QEvent::MouseButtonPress, {40, 40}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, {45, 45}, Qt::NoButton, Qt::LeftButton);
        QTRY_VERIFY(commands.count() >= 1);
        const auto preview = commands.last().at(0).toJsonObject();
        QCOMPARE(preview["cmd"].toString(), QString("retouch_preview"));
        QCOMPARE(preview["handle"].toString(), QString("target"));
        mouse(QEvent::MouseButtonRelease, {46, 45}, Qt::LeftButton, Qt::NoButton);
        const auto commit = commands.last().at(0).toJsonObject();
        QCOMPARE(commit["cmd"].toString(), QString("retouch_commit"));
        QCOMPARE(commit["session"], preview["session"]);
        QCOMPARE(commit["expected"], retouchGuide()["value"]);
        for (const auto cancel : {QEvent::KeyPress, QEvent::FocusOut, QEvent::UngrabMouse}) {
            commands.clear();
            mouse(QEvent::MouseButtonPress, {50, 40}, Qt::LeftButton, Qt::LeftButton);
            mouse(QEvent::MouseMove, {60, 40}, Qt::NoButton, Qt::LeftButton);
            if (cancel == QEvent::KeyPress) {
                QKeyEvent event(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
                QCoreApplication::sendEvent(&canvas, &event);
            } else {
                QEvent event(cancel);
                QCoreApplication::sendEvent(&canvas, &event);
            }
            QCOMPARE(commands.last().at(0).toJsonObject()["cmd"].toString(),
                     QString("retouch_cancel"));
            const auto count = commands.count();
            mouse(QEvent::MouseButtonRelease, {60, 40}, Qt::LeftButton, Qt::NoButton);
            QTest::qWait(60);
            QCOMPARE(commands.count(), count);
        }
        commands.clear();
        mouse(QEvent::MouseButtonPress, {10, 10}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, {15, 10}, Qt::NoButton, Qt::LeftButton);
        QTRY_VERIFY(commands.count() >= 1);
        QCOMPARE(commands.last().at(0).toJsonObject()["cmd"].toString(),
                 QString("retouch_placement"));
        mouse(QEvent::MouseButtonRelease, {15, 10}, Qt::LeftButton, Qt::NoButton);
        QCOMPARE(commands.last().at(0).toJsonObject()["tool"].toString(), QString("heal"));
        mouse(QEvent::MouseButtonPress, {40, 40}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, {45, 45}, Qt::NoButton, Qt::LeftButton);
        canvas.setRetouchState(99, 0,aigle::retouchGuidesFromJson( {}));
        const auto count = commands.count();
        mouse(QEvent::MouseButtonRelease, {45, 45}, Qt::LeftButton, Qt::NoButton);
        QCOMPARE(commands.count(), count);
    }
    void retouchCircleBodiesRimsAndOverlappingPlacement() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(200, 200);
        canvas.setExtent({200, 200});
        auto guide = retouchGuide();
        guide["target"] = QJsonObject{{"x", 50}, {"y", 50}};
        guide["source"] = QJsonObject{{"x", 160}, {"y", 120}};
        guide["radius_handle"] = QJsonObject{{"x", 80}, {"y", 50}};
        QJsonArray target, source;
        for (int i = 0; i <= 64; ++i) {
            const auto angle = i * 2 * 3.141592653589793 / 64;
            target.append(
                QJsonObject{{"x", 50 + 30 * std::cos(angle)}, {"y", 50 + 30 * std::sin(angle)}});
            source.append(
                QJsonObject{{"x", 160 + 30 * std::cos(angle)}, {"y", 120 + 30 * std::sin(angle)}});
        }
        guide["outline"] = target;
        guide["source_outline"] = source;
        canvas.setRetouchState(42, 7,aigle::retouchGuidesFromJson( QJsonArray{guide}));
        canvas.selectRetouch("spot", 0);
        canvas.setTool("retouch");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        const auto mouse = [&](QEvent::Type type, QPointF at,
                               Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
            const bool press = type == QEvent::MouseButtonPress;
            QMouseEvent event(type, at, at, Qt::LeftButton, press ? Qt::LeftButton : Qt::NoButton,
                              modifiers);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        mouse(QEvent::MouseButtonPress, {70, 50});
        mouse(QEvent::MouseButtonRelease, {72, 50});
        QCOMPARE(commands.last()[0].toJsonObject()["handle"].toString(), QString("target"));
        mouse(QEvent::MouseButtonPress, {20, 50});
        mouse(QEvent::MouseButtonRelease, {15, 50});
        QCOMPARE(commands.last()[0].toJsonObject()["handle"].toString(), QString("radius"));
        mouse(QEvent::MouseButtonPress, {140, 120});
        mouse(QEvent::MouseButtonRelease, {138, 120});
        QCOMPARE(commands.last()[0].toJsonObject()["handle"].toString(), QString("source"));
        mouse(QEvent::MouseButtonPress, {50, 50}, Qt::AltModifier);
        mouse(QEvent::MouseButtonRelease, {100, 50}, Qt::AltModifier);
        QCOMPARE(commands.last()[0].toJsonObject()["cmd"].toString(), QString("tool"));
    }
    void agentProvidersKeepModelsAndSessionKeysSeparate() {
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage.path());
        QSettings().setValue("agent/endpoint", "https://first.invalid/v1");
        QSettings().setValue("agent/model", "first-model");
        aigle::AgentPanel panel;
        QSignalSpy commands(&panel, &aigle::AgentPanel::command);
        auto* choice = panel.findChild<QComboBox*>("agentProviderChoice");
        QVERIFY(choice);
        QCOMPARE(choice->count(), 1);
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QPushButton*>("agentAddProvider")->click();
            dialog->findChild<QLineEdit*>("agentProviderName")->setText("Second provider");
            dialog->findChild<QLineEdit*>("agentEndpoint")->setText("https://second.invalid/v1");
            dialog->findChild<QLineEdit*>("agentModel")->setText("second-model");
            dialog->findChild<QLineEdit*>("agentApiKey")->setText("second-session-secret");
            dialog->findChild<QCheckBox*>("agentRememberKey")->setChecked(false);
            dialog->findChild<QDialogButtonBox*>()->button(QDialogButtonBox::Save)->click();
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(choice->count(), 2);
        QCOMPARE(choice->currentText(), QString("Second provider"));
        panel.setPhoto("/photos/providers.nef");
        panel.setState(aigle::agentStateFromJson({}), true);
        auto* prompt = panel.findChild<QPlainTextEdit*>("agentPrompt");
        const auto send = [&] {
            prompt->setPlainText("Test provider");
            QTest::keyClick(prompt, Qt::Key_Return);
            return commands.last().at(0).toJsonObject();
        };
        QCOMPARE(send().value("api_key").toString(), QString("second-session-secret"));
        choice->setCurrentIndex(0);
        const auto first = send();
        QCOMPARE(first.value("endpoint").toString(), QString("https://first.invalid/v1"));
        QCOMPARE(first.value("model").toString(), QString("first-model"));
        QVERIFY(first.value("api_key").toString().isEmpty());
        choice->setCurrentIndex(1);
        QCOMPARE(send().value("api_key").toString(), QString("second-session-secret"));
        for (const auto& key : QSettings().allKeys())
            QVERIFY(!QSettings().value(key).toByteArray().contains("second-session-secret"));
        aigle::AgentPanel restarted;
        QCOMPARE(restarted.findChild<QComboBox*>("agentProviderChoice")->currentText(),
                 QString("Second provider"));
        restarted.setPhoto("/photos/providers.nef");
        restarted.setState(aigle::agentStateFromJson({}), true);
        QSignalSpy restartedCommands(&restarted, &aigle::AgentPanel::command);
        auto* restartedPrompt = restarted.findChild<QPlainTextEdit*>("agentPrompt");
        restartedPrompt->setPlainText("Restart test");
        QTest::keyClick(restartedPrompt, Qt::Key_Return);
        const auto restored = restartedCommands.last().at(0).toJsonObject();
        QCOMPARE(restored.value("model").toString(), QString("second-model"));
        QVERIFY(restored.value("api_key").toString().isEmpty());
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            auto* remove = dialog->findChild<QPushButton*>("agentRemoveProvider");
            remove->click();
            const auto request = commands.last().at(0).toJsonObject();
            QCOMPARE(request.value("action").toString(), QString("delete_provider"));
            QCOMPARE(request.value("endpoint").toString(), QString("https://second.invalid/v1"));
            emit panel.credentialFinished(aigle::credentialResultFromJson(
                {{"request_id", request.value("request_id")}, {"error", "Locked"}}));
            QCOMPARE(choice->count(), 2);
            QVERIFY(dialog->isVisible());
            remove->click();
            emit panel.credentialFinished(aigle::credentialResultFromJson(
                {{"request_id", request.value("request_id")}, {"stored", false}}));
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(choice->count(), 1);
        QCOMPARE(send().value("endpoint").toString(), QString("https://first.invalid/v1"));
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QPushButton*>("agentAddProvider")->click();
            dialog->findChild<QLineEdit*>("agentProviderName")->setText("Cancelled");
            dialog->reject();
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(choice->count(), 1);
    }
    void agentDiscoveryHandlesResultsErrorsAndCancellation() {
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage.path());
        QSettings().setValue("agent/endpoint", "https://discovery.invalid/v1");
        QSettings().setValue("agent/model", "existing-model");
        aigle::AgentPanel panel;
        QSignalSpy commands(&panel, &aigle::AgentPanel::command);
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QPushButton*>("agentDiscoverModels")->click();
            const auto request = commands.last().at(0).toJsonObject();
            QCOMPARE(request.value("cmd").toString(), QString("agent_models"));
            auto* buttons = dialog->findChild<QDialogButtonBox*>();
            QVERIFY(buttons->button(QDialogButtonBox::Cancel)->isEnabled());
            QVERIFY(!buttons->button(QDialogButtonBox::Save)->isEnabled());
            emit panel.modelsFinished(aigle::modelDiscoveryFromJson(
                {{"request_id", "stale"}, {"models", QJsonArray{"wrong-model"}}}));
            QVERIFY(!buttons->button(QDialogButtonBox::Save)->isEnabled());
            emit panel.modelsFinished(aigle::modelDiscoveryFromJson(
                {{"request_id", request.value("request_id")},
                 {"models", QJsonArray{"vision-a", "vision-b", "vision-a", ""}}}));
            QVERIFY(buttons->button(QDialogButtonBox::Save)->isEnabled());
            auto* models = dialog->findChild<QComboBox*>("agentModels");
            QCOMPARE(models->count(), 2);
            QCOMPARE(models->currentText(), QString("existing-model"));
            models->setCurrentIndex(1);
            buttons->button(QDialogButtonBox::Save)->click();
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(QSettings().value("agent/model").toString(), QString("vision-b"));
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            auto* models = dialog->findChild<QComboBox*>("agentModels");
            QCOMPARE(models->count(), 2);
            dialog->findChild<QPushButton*>("agentDiscoverModels")->click();
            const auto request = commands.last().at(0).toJsonObject();
            emit panel.modelsFinished(aigle::modelDiscoveryFromJson(
                {{"request_id", request.value("request_id")}, {"error", "Unauthorized"}}));
            QVERIFY(dialog->findChild<QLabel*>("agentCredentialStatus")
                        ->text()
                        .contains("Unauthorized"));
            models->setEditText("manual-vision");
            dialog->findChild<QDialogButtonBox*>()->button(QDialogButtonBox::Save)->click();
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(QSettings().value("agent/model").toString(), QString("manual-vision"));
        QString cancelledId;
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QPushButton*>("agentDiscoverModels")->click();
            cancelledId = commands.last().at(0).toJsonObject().value("request_id").toString();
            dialog->reject();
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(commands.last().at(0).toJsonObject().value("cmd").toString(),
                 QString("agent_models_cancel"));
        QCOMPARE(commands.last().at(0).toJsonObject().value("request_id").toString(), cancelledId);
        emit panel.modelsFinished(aigle::modelDiscoveryFromJson(
            {{"request_id", cancelledId}, {"models", QJsonArray{"late-model"}}}));
        QCOMPARE(QSettings().value("agent/model").toString(), QString("manual-vision"));
    }
    void agentSessionKeyAndDeletionAreExplicit() {
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage.path());
        QSettings().setValue("agent/endpoint", "https://session.invalid/v1");
        QSettings().setValue("agent/model", "model");
        aigle::AgentPanel panel;
        QSignalSpy commands(&panel, &aigle::AgentPanel::command);
        auto* provider = panel.findChild<QToolButton*>();
        QVERIFY(provider);
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QLineEdit*>("agentApiKey")->setText("session-test-key");
            dialog->findChild<QCheckBox*>("agentRememberKey")->setChecked(false);
            dialog->findChild<QDialogButtonBox*>()->button(QDialogButtonBox::Save)->click();
        });
        provider->click();
        QCOMPARE(commands.count(), 0);
        panel.setPhoto("/photos/session.nef");
        panel.setState(aigle::agentStateFromJson({}), true);
        auto* prompt = panel.findChild<QPlainTextEdit*>("agentPrompt");
        prompt->setPlainText("Session test");
        QTest::keyClick(prompt, Qt::Key_Return);
        QCOMPARE(commands.last().at(0).toJsonObject().value("api_key").toString(),
                 QString("session-test-key"));
        commands.clear();
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QLineEdit*>("agentEndpoint")->setText("https://cancelled.invalid/v1");
            dialog->reject();
        });
        provider->click();
        QCOMPARE(commands.count(), 0);
        QCOMPARE(QSettings().value("agent/endpoint").toString(),
                 QString("https://session.invalid/v1"));
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            dialog->findChild<QCheckBox*>("agentForgetKey")->setChecked(true);
            dialog->findChild<QDialogButtonBox*>()->button(QDialogButtonBox::Save)->click();
            QCOMPARE(commands.count(), 1);
            const auto request = commands.last().at(0).toJsonObject();
            QCOMPARE(request.value("action").toString(), QString("delete"));
            QVERIFY(!request.contains("api_key"));
            emit panel.credentialFinished(aigle::credentialResultFromJson(
                {{"request_id", request.value("request_id")}, {"stored", false}}));
        });
        provider->click();
        prompt->setPlainText("After deletion");
        QTest::keyClick(prompt, Qt::Key_Return);
        QVERIFY(commands.last().at(0).toJsonObject().value("api_key").toString().isEmpty());
    }
    void agentKeyringFailureDoesNotSaveProviderOrSecret() {
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage.path());
        QSettings settings;
        settings.setValue("agent/endpoint", "https://old.invalid/v1");
        settings.setValue("agent/model", "old-model");
        aigle::AgentPanel panel;
        QSignalSpy commands(&panel, &aigle::AgentPanel::command);
        QTimer::singleShot(0, &panel, [&] {
            auto* dialog = panel.findChild<QDialog*>("agentProviderDialog");
            QVERIFY(dialog);
            auto* endpoint = dialog->findChild<QLineEdit*>("agentEndpoint");
            auto* model = dialog->findChild<QLineEdit*>("agentModel");
            auto* key = dialog->findChild<QLineEdit*>("agentApiKey");
            auto* buttons = dialog->findChild<QDialogButtonBox*>();
            endpoint->setText("https://new.invalid/v1");
            model->setText("new-model");
            key->setText("test-secret-never-in-settings");
            buttons->button(QDialogButtonBox::Save)->click();
            QCOMPARE(commands.count(), 1);
            QVERIFY(!buttons->isEnabled());
            QTest::keyClick(dialog, Qt::Key_Escape);
            QVERIFY(dialog->isVisible());
            const auto request = commands.at(0).at(0).toJsonObject();
            QCOMPARE(request.value("action").toString(), QString("save"));
            QCOMPARE(request.value("endpoint").toString(), QString("https://new.invalid/v1"));
            emit panel.credentialFinished(aigle::credentialResultFromJson({{"request_id", "unrelated"}, {"stored", true}}));
            QVERIFY(!buttons->isEnabled());
            emit panel.credentialFinished(aigle::credentialResultFromJson(
                {{"request_id", request.value("request_id")}, {"error", "Keyring locked"}}));
            QVERIFY(buttons->isEnabled());
            QVERIFY(dialog->isVisible());
            QVERIFY(dialog->findChild<QLabel*>("agentCredentialStatus")
                        ->text()
                        .contains("Keyring locked"));
            QCOMPARE(QSettings().value("agent/endpoint").toString(),
                     QString("https://old.invalid/v1"));
            buttons->button(QDialogButtonBox::Save)->click();
            QCOMPARE(commands.count(), 2);
            emit panel.credentialFinished(aigle::credentialResultFromJson(
                {{"request_id", request.value("request_id")}, {"stored", true}}));
        });
        panel.findChild<QToolButton*>()->click();
        QCOMPARE(QSettings().value("agent/endpoint").toString(), QString("https://new.invalid/v1"));
        QCOMPARE(QSettings().value("agent/model").toString(), QString("new-model"));
        for (const auto& name : QSettings().allKeys())
            QVERIFY(!QSettings().value(name).toString().contains("test-secret-never-in-settings"));
        panel.setPhoto("/photos/test.nef");
        panel.setState(aigle::agentStateFromJson({}), true);
        auto* prompt = panel.findChild<QPlainTextEdit*>("agentPrompt");
        prompt->setPlainText("Test saved credential");
        QTest::keyClick(prompt, Qt::Key_Return);
        QCOMPARE(commands.last().at(0).toJsonObject().value("api_key").toString(), QString());
    }
    void chineseTranslationsCoverWidgetsAndDynamicLabels() {
        QTranslator translator;
        QVERIFY(translator.load(":/i18n/aigle_zh_CN.qm"));
        qApp->installTranslator(&translator);
        const auto restore = qScopeGuard([&] { qApp->removeTranslator(&translator); });
        QCOMPARE(aigle::uiText("Exposure"), QString::fromUtf8("曝光"));
        QCOMPARE(aigle::uiText("RAW & profiles"), QString::fromUtf8("RAW 与配置文件"));
        QCOMPARE(QCoreApplication::translate("aigle::Window", "Importing: %1 / %2 files").arg(3).arg(8),
            QString::fromUtf8("正在导入：3 / 8 个文件"));
        const QJsonArray formats{QJsonObject{{"name", "JPEG"}, {"extension", "jpg"},
            {"format", QJsonObject{{"Jpeg", QJsonObject{{"quality", 90}}}}}}};
        aigle::ExportDialog dialog("/tmp/User photo.NEF", QImage(), QSize(6000, 4000),aigle::exportFormatsFromJson( formats));
        QCOMPARE(dialog.windowTitle(), QString::fromUtf8("导出照片"));
        QVERIFY(exportRequestsJson(dialog.requests())[0].toObject()["options"].toObject()["path"].toString().contains("User photo"));
        aigle::ToolPanel masks;
        masks.setPage("masks");
        bool brush = false;
        for (auto* button : masks.findChildren<QPushButton*>()) brush |= button->text() == QString::fromUtf8("画笔");
        QVERIFY(brush);
    }
    void exportLiveTranslationPreservesStagedRequest() {
        aigle::applyTranslations(*qApp, "en");
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        const QJsonArray formats{QJsonObject{{"name", "JPEG"}, {"extension", "jpg"},
            {"format", QJsonObject{{"Jpeg", QJsonObject{{"quality", 90}}}}}}};
        aigle::ExportDialog dialog("/tmp/User photo.NEF", {}, {6000, 4000},aigle::exportFormatsFromJson( formats));
        auto* text = dialog.findChild<QLineEdit*>("exportWatermarkText");
        QVERIFY(text);
        text->setText("My original text");
        const auto before = exportRequestsJson(dialog.requests());
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(dialog.windowTitle(), QString::fromUtf8("导出照片"));
        QCOMPARE(exportRequestsJson(dialog.requests()), before);
        QCOMPARE(text->text(), QString("My original text"));
        bool cancel = false;
        for (auto* button : dialog.findChildren<QPushButton*>())
            cancel |= button->text() == QString::fromUtf8("取消");
        QVERIFY(cancel);
        aigle::applyTranslations(*qApp, "en");
        QCOMPARE(dialog.windowTitle(), QString("Export photo"));
        QCOMPARE(exportRequestsJson(dialog.requests()), before);
    }
    void exportValidationRetranslatesAndCanRecover() {
        aigle::applyTranslations(*qApp, "en");
        QSettings settings;
        const auto oldLast = settings.value("export/lastSettings");
        const auto oldDestination = settings.value("export/destination");
        const auto restore = qScopeGuard([&] {
            aigle::applyTranslations(*qApp, "en");
            if (oldLast.isValid()) settings.setValue("export/lastSettings", oldLast); else settings.remove("export/lastSettings");
            if (oldDestination.isValid()) settings.setValue("export/destination", oldDestination); else settings.remove("export/destination");
        });
        QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const QJsonArray formats{QJsonObject{{"name", "PNG"}, {"extension", "png"}, {"format", "Png"}}};
        aigle::ExportDialog dialog("/tmp/translation-check.png", {}, {100, 100},aigle::exportFormatsFromJson( formats));
        dialog.findChild<QComboBox*>("exportFormat")->setCurrentIndex(0);
        dialog.findChild<QLineEdit*>("exportDestination")->setText(directory.path());
        dialog.findChild<QComboBox*>("exportWatermarkKind")->setCurrentIndex(1);
        auto* input = dialog.findChild<QLineEdit*>("exportWatermarkText");
        input->clear();
        auto* submit = dialog.findChild<QPushButton*>("primary");
        submit->click();
        auto* error = dialog.findChild<QLabel*>("exportError");
        QCOMPARE(error->text(), QString("Enter watermark text or select None."));
        QVERIFY(!error->isHidden());
        const auto request = exportRequestsJson(dialog.requests());
        aigle::applyTranslations(*qApp, "zh-CN");
        QVERIFY(error->text() != QString("Enter watermark text or select None."));
        QCOMPARE(exportRequestsJson(dialog.requests()), request);
        aigle::applyTranslations(*qApp, "en");
        QCOMPARE(error->text(), QString("Enter watermark text or select None."));
        input->setText("Valid watermark");
        QSignalSpy accepted(&dialog, &QDialog::accepted);
        submit->click();
        QCOMPARE(accepted.size(), 1);
    }
    void dialogShortcutsSupportChordsAndSuppressOldCancel() {
        QSettings settings;
        const auto oldAccept = settings.value("shortcuts/dialog_accept");
        const auto oldCancel = settings.value("shortcuts/dialog_cancel");
        const auto restore = qScopeGuard([&] {
            if (oldAccept.isValid()) settings.setValue("shortcuts/dialog_accept", oldAccept); else settings.remove("shortcuts/dialog_accept");
            if (oldCancel.isValid()) settings.setValue("shortcuts/dialog_cancel", oldCancel); else settings.remove("shortcuts/dialog_cancel");
        });
        settings.setValue("shortcuts/dialog_accept", "Ctrl+K, Ctrl+Return");
        settings.setValue("shortcuts/dialog_cancel", "Ctrl+Escape");
        QDialog dialog;
        QVBoxLayout layout(&dialog);
        QLineEdit input;
        QDialogButtonBox buttons(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
        layout.addWidget(&input);
        layout.addWidget(&buttons);
        connect(&buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
        connect(&buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
        aigle::installDialogShortcuts(&dialog);
        dialog.show();
        dialog.activateWindow();
        input.setFocus();
        QTest::qWait(20);
        QSignalSpy accepted(&dialog, &QDialog::accepted), rejected(&dialog, &QDialog::rejected);
        QTest::keyClick(&input, Qt::Key_Escape);
        QCOMPARE(rejected.size(), 0);
        QTest::keyClicks(&input, "Keep text");
        QCOMPARE(input.text(), QString("Keep text"));
        QTest::keyClick(&input, Qt::Key_K, Qt::ControlModifier);
        QTest::keyClick(&input, Qt::Key_Return, Qt::ControlModifier);
        QCOMPARE(accepted.size(), 1);
    }

    void typedViewportQueuedDeliveryAndWireFormat() {
        aigle::Canvas canvas(nullptr);
        QObject receiver;
        std::optional<aigle::ViewportRequest> received;
        QObject::connect(&canvas, &aigle::Canvas::viewportChanged, &receiver,
            [&](const aigle::ViewportRequest& value) { received = value; }, Qt::QueuedConnection);
        const aigle::ViewportRequest value{{-12.5, 8, 1200, 800}, {640, 480},
            {aigle::DisplayProfile::Kind::Icc, "monitor.icc"}, true};
        emit canvas.viewportChanged(value);
        QVERIFY(!received);
        QTRY_VERIFY(received.has_value());
        QCOMPARE(*received, value);
        const auto wire = aigle::toJson(*received);
        QCOMPARE(wire["cmd"].toString(), QString("viewport"));
        QVERIFY(wire["continuous"].toBool());
        const auto viewport = wire["viewport"].toObject();
        QCOMPARE(viewport["region"].toObject()["x"].toDouble(), -12.5);
        QCOMPARE(viewport["output_size"].toArray(), QJsonArray({640, 480}));
        QCOMPARE(viewport["display"].toObject()["Icc"].toString(), QString("monitor.icc"));
        QCOMPARE(aigle::toJson(aigle::DisplayProfile{aigle::DisplayProfile::Kind::HdrRec2020, {}}),
                 QJsonValue("HdrRec2020"));
    }

    void typedDisplayDetectionHandlesMissingAndInvalidProfiles() {
        QVERIFY(!aigle::displayDetectionFromJson({}).profile);
        QVERIFY(!aigle::displayDetectionFromJson({{"profile", QJsonObject{{"Icc", 42}}}}).profile);
        const auto value = aigle::displayDetectionFromJson({{"running", true}, {"screen", "screen-1"},
            {"name", "Monitor"}, {"profile", QJsonObject{{"Icc", "profile-data"}}}});
        QVERIFY(value.running);
        QCOMPARE(value.screen, QString("screen-1"));
        QVERIFY(value.profile);
        QCOMPARE(value.profile->kind, aigle::DisplayProfile::Kind::Icc);
        QCOMPARE(value.profile->icc, QString("profile-data"));
    }

    void canvasDisplayProfileControlsSdrViewportRequests() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(640, 480);
        canvas.setExtent({1200, 800});
        QSignalSpy requests(&canvas, &aigle::Canvas::viewportChanged);
        canvas.setDisplayProfile({aigle::DisplayProfile::Kind::Icc, "test-profile"}, "Test monitor");
        QVERIFY(!requests.isEmpty());
        QCOMPARE(requests.last()[0].value<aigle::ViewportRequest>().display,
            (aigle::DisplayProfile{aigle::DisplayProfile::Kind::Icc, "test-profile"}));
        const int before = requests.size();
        canvas.setDisplayProfile({aigle::DisplayProfile::Kind::Icc, "test-profile"}, "Test monitor");
        QCOMPARE(requests.size(), before);
        canvas.setDisplayProfile({}, "sRGB");
        QCOMPARE(requests.last()[0].value<aigle::ViewportRequest>().display.kind, aigle::DisplayProfile::Kind::Srgb);
    }

    void agentRichEventsPersistAcrossMoveAndDoNotDuplicateOnPoll() {
        const auto identity = "test-rich-" + QUuid::createUuid().toString();
        QImage preview(8, 8, QImage::Format_RGB32);
        preview.fill(Qt::red);
        QByteArray bytes;
        QBuffer buffer(&bytes);
        buffer.open(QIODevice::WriteOnly);
        QVERIFY(preview.save(&buffer, "PNG"));
        const QJsonArray events{
            QJsonObject{{"role", "assistant"}, {"text", "**Warm light**"},
                {"profile", QJsonObject{{"light.exposure", .5}}}},
            QJsonObject{{"role", "assistant"}, {"text", ""}, {"image", QString::fromLatin1(bytes.toBase64())}}};
        const QJsonObject state{{"running", true}, {"run_id", 7}, {"events", events}};
        aigle::AgentPanel panel;
        panel.setPhoto("/photos/before.nef", {}, identity);
        panel.setState(aigle::agentStateFromJson(state), true);
        panel.setState(aigle::agentStateFromJson(state), true);
        auto* transcript = panel.findChild<QTextBrowser*>("agentTranscript");
        QCOMPARE(transcript->toPlainText().count("Warm light"), 1);
        QVERIFY(transcript->toHtml().contains("agent-preview:1"));
        QVERIFY(QMetaObject::invokeMethod(transcript, "anchorClicked", Q_ARG(QUrl, QUrl("profile:0"))));
        QVERIFY(transcript->toPlainText().contains("light.exposure"));
        {
            const auto restoreLanguage = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            const auto saved = QSettings().allKeys();
            QMap<QString, QVariant> conversations;
            for (const auto& key : saved)
                if (key.startsWith("agent/conversations/")) conversations.insert(key, QSettings().value(key));
            aigle::applyTranslations(*qApp, "zh-CN");
            QVERIFY(transcript->toPlainText().contains(aigle::AgentPanel::tr("Hide adjustments")));
            QVERIFY(!transcript->toPlainText().contains("Hide adjustments"));
            QCOMPARE(transcript->toPlainText().count("Warm light"), 1);
            QVERIFY(transcript->toPlainText().contains("light.exposure"));
            QVERIFY(transcript->toHtml().contains("agent-preview:1"));
            for (auto it = conversations.begin(); it != conversations.end(); ++it)
                QCOMPARE(QSettings().value(it.key()), it.value());
        }
        panel.setPhoto("/moved/after.nef", {}, identity);
        QVERIFY(transcript->toPlainText().contains("Warm light"));
        aigle::AgentPanel reopened;
        reopened.setPhoto("/moved/after.nef", {}, identity);
        auto* restored = reopened.findChild<QTextBrowser*>("agentTranscript");
        QVERIFY(restored->toHtml().contains("agent-preview:1"));
        QVERIFY(restored->toPlainText().contains("Warm light"));
        reopened.setPhoto("/moved/after.nef", "42", identity);
        QVERIFY(!restored->toPlainText().contains("Warm light"));
    }

    void agentConversationPersistsAndStaysWithPhoto() {
        QTemporaryDir storage;
        QVERIFY(storage.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage.path());
        QSettings settings;
        settings.setValue("agent/endpoint", "https://example.invalid/v1");
        settings.setValue("agent/model", "test-model");
        {
            aigle::AgentPanel panel;
            panel.setPhoto("/photos/a.nef");
            panel.setState(aigle::agentStateFromJson({}), true);
            auto* prompt = panel.findChild<QPlainTextEdit*>("agentPrompt");
            auto* transcript = panel.findChild<QTextBrowser*>("agentTranscript");
            QVERIFY(prompt && transcript);
            QSignalSpy commands(&panel, &aigle::AgentPanel::command);
            prompt->setPlainText("Warm <this> image");
            QTest::keyClick(prompt, Qt::Key_Return);
            QCOMPARE(commands.count(), 1);
            QCOMPARE(commands.at(0).at(0).toJsonObject().value("prompt").toString(),
                     QString("User: Warm <this> image"));
            panel.setState(aigle::agentStateFromJson({{"running", true}, {"message", "Temporary progress"}}), true);
            QVERIFY(!transcript->toPlainText().contains("Temporary progress"));
            const QJsonObject done{{"running", false}, {"message", "Warmer highlights"}};
            panel.setState(aigle::agentStateFromJson(done), true);
            panel.setState(aigle::agentStateFromJson(done), true);
            QCOMPARE(transcript->toPlainText().count("Warmer highlights"), 1);
            panel.setPhoto("/photos/b.nef");
            QVERIFY(!transcript->toPlainText().contains("Warmer highlights"));
            panel.setPhoto("/photos/a.nef", "42");
            QVERIFY(!transcript->toPlainText().contains("Warmer highlights"));
        }
        aigle::AgentPanel reopened;
        reopened.setPhoto("/photos/a.nef");
        reopened.setState(aigle::agentStateFromJson({}), true);
        auto* transcript = reopened.findChild<QTextBrowser*>("agentTranscript");
        QVERIFY(transcript->toPlainText().contains("Warm <this> image"));
        QVERIFY(transcript->toPlainText().contains("Warmer highlights"));
        QSignalSpy commands(&reopened, &aigle::AgentPanel::command);
        auto* prompt = reopened.findChild<QPlainTextEdit*>("agentPrompt");
        prompt->setPlainText("Less warm");
        QTest::keyClick(prompt, Qt::Key_Return);
        QCOMPARE(commands.count(), 1);
        QCOMPARE(
            commands.at(0).at(0).toJsonObject().value("prompt").toString(),
            QString("User: Warm <this> image\n\nAssistant: Warmer highlights\n\nUser: Less warm"));
        reopened.setPhoto({});
        QVERIFY(transcript->toPlainText().isEmpty());
    }
    void maskCoverageSwatchSurvivesListRebuild() {
        aigle::ToolPanel editor;
        QJsonArray coverage;
        for (int i = 0; i < 1024; ++i)
            coverage.append(i % 256);
        editor.setGuides(aigle::maskGuidesFromJson({QJsonObject{{"id", 7}, {"coverage", coverage}}}));
        editor.setParams(
            {{"masks",
              QJsonArray{QJsonObject{
                  {"id", 7}, {"name", "Coverage"}, {"shape", QJsonObject{{"kind", "brush"}}}}}}});
        editor.setPage("masks");
        auto* list = editor.findChild<QListWidget*>();
        QVERIFY(list);
        QCOMPARE(list->count(), 1);
        const auto image = qvariant_cast<QImage>(list->item(0)->data(Qt::DecorationRole));
        QCOMPARE(image.size(), QSize(32, 32));
        QCOMPARE(image.constScanLine(0)[17], uchar(17));
    }
    void cancelledMaskDragCannotCommitOnRelease() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(100, 100);
        canvas.setExtent({100, 100});
        canvas.setTool("mask_edit");
        canvas.setMaskGuide(aigle::maskGuideFromJson({{"id", 1},
                             {"kind", "move"},
                             {"from", QJsonObject{{"x", 50}, {"y", 50}}},
                             {"to", QJsonObject{{"x", 50}, {"y", 50}}}}));
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        auto mouse = [&](QEvent::Type type, QPointF at, Qt::MouseButton button,
                         Qt::MouseButtons buttons) {
            QMouseEvent event(type, at, at, button, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        for (const auto cancel : {QEvent::KeyPress, QEvent::FocusOut, QEvent::UngrabMouse}) {
            mouse(QEvent::MouseButtonPress, {50, 50}, Qt::LeftButton, Qt::LeftButton);
            mouse(QEvent::MouseMove, {70, 60}, Qt::NoButton, Qt::LeftButton);
            if (cancel == QEvent::KeyPress) {
                QKeyEvent event(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
                QCoreApplication::sendEvent(&canvas, &event);
            } else {
                QEvent event(cancel);
                QCoreApplication::sendEvent(&canvas, &event);
            }
            mouse(QEvent::MouseButtonRelease, {70, 60}, Qt::LeftButton, Qt::NoButton);
            QCOMPARE(commands.size(), 0);
        }
        mouse(QEvent::MouseButtonPress, {50, 50}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, {60, 50}, Qt::LeftButton, Qt::NoButton);
        QCOMPARE(commands.size(), 1);
        QCOMPARE(commands[0][0].toJsonObject()["handle"].toString(), QString("move"));
    }
    void cropOrientationButtonsStageWithoutEditingDocument() {
        aigle::ToolPanel editor;
        editor.setPage("crop");
        QSignalSpy operations(&editor, &aigle::ToolPanel::cropOrientationChanged);
        QSignalSpy commands(&editor, &aigle::ToolPanel::command);
        const QStringList labels{"Rotate left", "Rotate right", "Flip horizontal", "Flip vertical"};
        for (int i = 0; i < labels.size(); ++i) {
            QPushButton* button = nullptr;
            for (auto* candidate : editor.findChildren<QPushButton*>())
                if (candidate->text() == labels[i])
                    button = candidate;
            QVERIFY(button);
            button->click();
            QCOMPARE(operations.last()[0].toInt(), i);
        }
        QCOMPARE(operations.size(), 4);
        QCOMPARE(commands.size(), 0);
    }
    void cropInspectorAngleUpdatesDuringDragAndRegrowsFromAnchor() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 400);
        canvas.setExtent({400, 400});
        canvas.setTool("crop");
        aigle::ToolPanel panel;
        panel.setPage("crop");
        connect(&panel, &aigle::ToolPanel::cropAngleChanged, &canvas, &aigle::Canvas::setCropAngle);
        connect(&panel, &aigle::ToolPanel::cropAngleGestureStarted, &canvas, &aigle::Canvas::beginCropAngleGesture);
        connect(&panel, &aigle::ToolPanel::cropAngleGestureFinished, &canvas, &aigle::Canvas::endCropAngleGesture);
        QSlider* slider = nullptr;
        for (auto* candidate : panel.findChildren<QSlider*>())
            if (candidate->accessibleName() == "Angle") slider = candidate;
        QVERIFY(slider);
        QSignalSpy changes(&panel, &aigle::ToolPanel::cropAngleChanged);
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        slider->setSliderDown(true);
        slider->setValue(750); // 30 degrees, still held.
        QCOMPARE(changes.size(), 1);
        QCOMPARE(changes.last()[0].toDouble(), 30.);
        QCOMPARE(commands.size(), 0);
        slider->setValue(450); // Return to zero before release.
        QCOMPARE(changes.last()[0].toDouble(), 0.);
        slider->setSliderDown(false);
        QCOMPARE(commands.size(), 0);
        canvas.applyCrop();
        const auto points = commands.last()[0].toJsonObject()["points"].toArray();
        QCOMPARE(points[0].toObject()["x"].toDouble(), 0.);
        QCOMPARE(points[1].toObject()["x"].toDouble(), 1.);
    }
    void cropExtentUpdateDoesNotMaximizeSavedCrop() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 200);
        canvas.setExtent({400, 200});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{
            {"crop", QJsonObject{{"x", .1}, {"y", .3}, {"w", .2}, {"h", .4}}},
            {"aspect", QJsonObject{{"kind", "ratio"}, {"w", 1}, {"h", 1}}}}}}));
        canvas.setTool("crop");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        canvas.setExtent({800, 400});
        canvas.applyCrop();
        const auto points = commands.last()[0].toJsonObject()["points"].toArray();
        QVERIFY(std::abs(points[0].toObject()["x"].toDouble() - .1) < .0001);
        QVERIFY(std::abs(points[1].toObject()["x"].toDouble() - .3) < .0001);
    }
    void cropAspectExpandsAroundSavedCenter() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 200);
        canvas.setExtent({400, 200});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{{"crop", QJsonObject{
            {"x", .1}, {"y", .3}, {"w", .2}, {"h", .4}}}}}}));
        canvas.setTool("crop");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        canvas.setCropAspect(1);
        QCOMPARE(commands.size(), 0);
        canvas.applyCrop();
        auto points = commands.last()[0].toJsonObject()["points"].toArray();
        const auto a = points[0].toObject(), b = points[1].toObject();
        // Center (80,100), largest centered square has a 160px side.
        QVERIFY(std::abs(a["x"].toDouble()) < .0001);
        QVERIFY(std::abs(a["y"].toDouble() - .1) < .0001);
        QVERIFY(std::abs(b["x"].toDouble() - .4) < .0001);
        QVERIFY(std::abs(b["y"].toDouble() - .9) < .0001);
        canvas.setCropAspect(0);
        canvas.applyCrop();
        QCOMPARE(commands.last()[0].toJsonObject()["points"].toArray(), points);
        canvas.setCropAspect(-1);
        canvas.applyCrop();
        points = commands.last()[0].toJsonObject()["points"].toArray();
        QVERIFY(std::abs(points[0].toObject()["y"].toDouble() - .3) < .0001);
        QVERIFY(std::abs(points[1].toObject()["y"].toDouble() - .7) < .0001);
    }
    void cropAspectCommitsOnlyOnApply() {
        aigle::Canvas canvas(nullptr);
        canvas.setExtent({1200, 800});
        canvas.setTool("crop");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        canvas.setCropAspect(4. / 5);
        QCOMPARE(commands.size(), 0);
        canvas.cancelCrop();
        QCOMPARE(commands.size(), 0);
        canvas.setTool("pan");
        canvas.setTool("crop");
        canvas.setCropAspect(16. / 9);
        canvas.applyCrop();
        QCOMPARE(commands.size(), 1);
        QCOMPARE(commands.first()[0].toJsonObject()["aspect"].toObject(),
                 QJsonObject({{"kind", "ratio"}, {"w", 16}, {"h", 9}}));
    }
    void cropDoubleClickCompletesOnlyInsideBody() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 400);
        canvas.setExtent({400, 400});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{{"crop", QJsonObject{
            {"x", .2}, {"y", .2}, {"w", .6}, {"h", .6}}}}}}));
        canvas.setTool("crop");
        canvas.setCropAngle(7);
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        QSignalSpy finished(&canvas, &aigle::Canvas::toolFinished);
        const auto click = [&](QPointF at, Qt::MouseButton button) {
            QMouseEvent event(QEvent::MouseButtonDblClick, at, at, button, button, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        click({20, 20}, Qt::LeftButton);
        click({80, 80}, Qt::LeftButton);
        click({200, 80}, Qt::LeftButton);
        click({200, 200}, Qt::RightButton);
        QCOMPARE(commands.size(), 0);
        QCOMPARE(finished.size(), 0);
        click({200, 200}, Qt::LeftButton);
        QCOMPARE(commands.size(), 1);
        QCOMPARE(finished.size(), 1);
        QCOMPARE(commands.first()[0].toJsonObject()["angle"].toDouble(), 7.);
        QCOMPARE(canvas.navigation().zoom, 1.);
    }
    void cropEdgeResizeAndOutsideRotationStayStagedUntilApply() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(100, 100);
        canvas.setExtent({100, 100});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{{"crop", QJsonObject{
            {"x", 0.1}, {"y", 0.1}, {"w", 0.8}, {"h", 0.8}}}}}}));
        canvas.setTool("crop");
        canvas.setCropAspect(0);
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        auto mouse = [&](QEvent::Type type, QPointF at, Qt::MouseButton button,
                         Qt::MouseButtons buttons) {
            QMouseEvent event(type, at, at, button, buttons, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        mouse(QEvent::MouseButtonPress, {50, 10}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, {50, 20}, Qt::NoButton, Qt::LeftButton);
        QCOMPARE(commands.size(), 0);
        mouse(QEvent::MouseButtonRelease, {50, 20}, Qt::LeftButton, Qt::NoButton);
        mouse(QEvent::MouseButtonPress, {-15, 50}, Qt::LeftButton, Qt::LeftButton);
        mouse(QEvent::MouseMove, {-15, 60}, Qt::NoButton, Qt::LeftButton);
        mouse(QEvent::MouseButtonRelease, {-15, 60}, Qt::LeftButton, Qt::NoButton);
        QCOMPARE(commands.size(), 0);
        canvas.applyCrop();
        QCOMPARE(commands.size(), 1);
        auto request = commands.last()[0].toJsonObject();
        auto points = request["points"].toArray();
        QVERIFY(points[0].toObject()["y"].toDouble() > 0.1);
        QVERIFY(std::abs(points[0].toObject()["x"].toDouble() - 0.1) < 0.01);
        QVERIFY(std::abs(points[1].toObject()["x"].toDouble() - 0.9) < 0.01);
        QVERIFY(std::abs(request["angle"].toDouble()) > 1.0);
    }
    void cropRotationConstrainsDraftAndCancelledDragRestoresIt() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 400);
        canvas.setExtent({400, 400});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{{"crop", QJsonObject{
            {"x", 0}, {"y", 0}, {"w", 1}, {"h", 1}}}}}}));
        canvas.setTool("crop");
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        canvas.setCropAngle(30);
        QCOMPARE(commands.size(), 0);
        canvas.applyCrop();
        auto points = commands.last()[0].toJsonObject()["points"].toArray();
        const double side = points[1].toObject()["x"].toDouble() - points[0].toObject()["x"].toDouble();
        QVERIFY(side > .72 && side < .74); // Square inside a frame rotated 30 degrees.
        canvas.setTool("pan");
        canvas.setTool("crop");
        const auto mouse = [&](QEvent::Type type, QPointF at) {
            QMouseEvent event(type, at, at,
                type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton,
                Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        mouse(QEvent::MouseButtonPress, {-40, 200});
        mouse(QEvent::MouseMove, {-40, 100});
        QKeyEvent escape(QEvent::KeyPress, Qt::Key_Escape, Qt::NoModifier);
        QCoreApplication::sendEvent(&canvas, &escape);
        canvas.applyCrop();
        const auto result = commands.last()[0].toJsonObject();
        QCOMPARE(result["angle"].toDouble(), 0.);
        points = result["points"].toArray();
        QCOMPARE(points[0].toObject()["x"].toDouble(), 0.);
        QCOMPARE(points[1].toObject()["x"].toDouble(), 1.);
    }
    void cropResizePreservesGrabOffsetAndStopsAtOppositeEdge() {
        aigle::Canvas canvas(nullptr);
        canvas.resize(400, 200);
        canvas.setExtent({400, 200});
        canvas.setEditParams(aigle::geometryFromJson({{"geometry", QJsonObject{{"crop", QJsonObject{
            {"x", .1}, {"y", .1}, {"w", .8}, {"h", .8}}}}}}));
        canvas.setTool("crop");
        canvas.setCropAspect(0);
        QSignalSpy commands(&canvas, &aigle::Canvas::toolCommand);
        const auto mouse = [&](QEvent::Type type, QPointF at) {
            QMouseEvent event(type, at, at,
                type == QEvent::MouseMove ? Qt::NoButton : Qt::LeftButton,
                Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(&canvas, &event);
        };
        // Grab five pixels inside the west grip; zero motion must not jump.
        mouse(QEvent::MouseButtonPress, {45, 100});
        mouse(QEvent::MouseMove, {45, 100});
        mouse(QEvent::MouseButtonRelease, {45, 100});
        canvas.applyCrop();
        auto points = commands.last()[0].toJsonObject()["points"].toArray();
        QVERIFY(std::abs(points[0].toObject()["x"].toDouble() - .1) < .0001);
        canvas.setTool("crop");
        mouse(QEvent::MouseButtonPress, {45, 100});
        mouse(QEvent::MouseMove, {395, 100});
        mouse(QEvent::MouseButtonRelease, {395, 100});
        canvas.applyCrop();
        points = commands.last()[0].toJsonObject()["points"].toArray();
        const double l = points[0].toObject()["x"].toDouble();
        const double r = points[1].toObject()["x"].toDouble();
        QVERIFY(std::abs(r - .9) < .0001);
        QVERIFY(std::abs(l - .89) < .0001); // 4px = 2% of the 200px short side.
    }
    void cropRestoresSavedCustomAspectWithoutEdits() {
        aigle::ToolPanel editor;
        editor.setParams({{"geometry", QJsonObject{{"aspect", QJsonObject{
            {"kind", "ratio"}, {"w", 7}, {"h", 5}}}}}});
        QSignalSpy changes(&editor, &aigle::ToolPanel::aspectChanged);
        editor.setPage("crop");
        auto* aspect = editor.findChild<QComboBox*>("cropAspect");
        QVERIFY(aspect);
        QCOMPARE(aspect->currentText(), QString("7 : 5"));
        QCOMPARE(aspect->currentData().toDouble(), 1.4);
        QCOMPARE(changes.size(), 0);
        QVERIFY(QMetaObject::invokeMethod(aspect, "activated", Q_ARG(int, aspect->currentIndex())));
        QCOMPARE(changes.last()[0].toDouble(), 1.4);
    }
    void portraitCropPresetSurvivesUnlockAndTranslation() {
        aigle::ToolPanel editor;
        editor.setPage("crop");
        auto* aspect = editor.findChild<QComboBox*>("cropAspect");
        QVERIFY(aspect);
        const int portrait = aspect->findText("4 : 5");
        QVERIFY(portrait >= 0);
        QSignalSpy changes(&editor, &aigle::ToolPanel::aspectChanged);
        aspect->setCurrentIndex(portrait);
        QVERIFY(QMetaObject::invokeMethod(aspect, "activated", Q_ARG(int, portrait)));
        QCOMPARE(changes.last()[0].toDouble(), .8);
        QCheckBox* lock = nullptr;
        for (auto* check : editor.findChildren<QCheckBox*>())
            if (check->text() == "Lock aspect ratio") lock = check;
        QVERIFY(lock);
        lock->setChecked(false);
        QCOMPARE(changes.last()[0].toDouble(), 0.0);
        lock->setChecked(true);
        QCOMPARE(changes.last()[0].toDouble(), .8);
        const auto count = changes.size();
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(aspect->currentIndex(), portrait);
        QCOMPARE(aspect->currentText(), QString("4 : 5"));
        QCOMPARE(changes.size(), count);
    }
    void horizonFeedbackStaysStagedAndCropReopenRestoresSavedAngle() {
        aigle::ToolPanel editor;
        editor.setParams({{"geometry", QJsonObject{{"angle", 2.5}}}});
        editor.setPage("crop");
        QSignalSpy commands(&editor, &aigle::ToolPanel::command);
        QSignalSpy angles(&editor, &aigle::ToolPanel::cropAngleChanged);
        editor.setCropAngle(-7.5);
        auto* number = editor.findChild<QDoubleSpinBox*>();
        QVERIFY(number);
        QCOMPARE(number->value(), -7.5);
        QCOMPARE(commands.size(), 0);
        QCOMPARE(angles.size(), 0);
        const auto combos = editor.findChildren<QComboBox*>();
        QList<int> selected;
        for (auto* combo : combos) selected.append(combo->currentIndex());
        {
            const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            aigle::applyTranslations(*qApp, "zh-CN");
            QCOMPARE(editor.findChild<QDoubleSpinBox*>(), number);
            QCOMPARE(number->value(), -7.5);
            QCOMPARE(number->accessibleName(), aigle::ToolPanel::tr("Angle"));
            QVERIFY(number->accessibleName() != "Angle");
            for (int i = 0; i < combos.size(); ++i) QCOMPARE(combos[i]->currentIndex(), selected[i]);
            bool translated = false;
            for (auto* button : editor.findChildren<QPushButton*>())
                translated |= button->text() == aigle::ToolPanel::tr("Apply crop");
            QVERIFY(translated);
            QCOMPARE(commands.size(), 0);
            QCOMPARE(angles.size(), 0);
        }
        editor.setPage("masks");
        editor.setPage("crop");
        QCOMPARE(editor.findChild<QDoubleSpinBox*>()->value(), 2.5);
        QCOMPARE(editor.findChild<QDoubleSpinBox*>()->accessibleName(), QString("Angle"));
    }
    void whiteBalanceSectionResetRestoresAllParameters() {
        const QJsonObject properties{
            {"temperature", QJsonObject{{"type", "number"}, {"minimum", 2000}, {"maximum", 50000}, {"default", 6504}}},
            {"tint", QJsonObject{{"type", "number"}, {"minimum", -150}, {"maximum", 150}, {"default", 0}}},
            {"mode", QJsonObject{{"type", "string"}, {"default", "as_shot"}}}};
        aigle::Adjustments editor;
        editor.setSchema({{"properties", QJsonObject{{"wb", QJsonObject{{"properties", properties}}}}}});
        auto* reset = editor.findChild<QToolButton*>("resetSection.WhiteBalance");
        QVERIFY(reset);
        editor.setParams({{"wb", QJsonObject{{"temperature", 6504}, {"tint", 0}, {"mode", "as_shot"}}}});
        QVERIFY(reset->isHidden());
        // A mode-only change must also expose reset, even with default scalar values.
        editor.setParams({{"wb", QJsonObject{{"temperature", 6504}, {"tint", 0}, {"mode", "custom"}}}});
        QVERIFY(!reset->isHidden());
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        reset->click();
        QCOMPARE(commands.size(), 1);
        const auto ops = commands[0][0].toJsonObject()["ops"].toArray();
        QCOMPARE(ops.size(), 3);
        QStringList paths;
        for (const auto& op : ops) {
            QCOMPARE(op.toObject()["op"].toString(), QString("reset"));
            paths.append(op.toObject()["path"].toString());
        }
        QCOMPARE(paths, QStringList({"wb.temperature", "wb.tint", "wb.mode"}));
    }
    void whiteBalanceEditsPreserveAsShotCompanion() {
        const auto number = [](double minimum, double maximum) {
            return QJsonObject{
                {"type", "number"}, {"minimum", minimum}, {"maximum", maximum}, {"x-step", 1}};
        };
        const QJsonObject wb{{"type", "object"},
                             {"properties", QJsonObject{{"temperature", number(2000, 50000)},
                                                        {"tint", number(-150, 150)}}}};
        const QJsonObject schema{{"type", "object"}, {"properties", QJsonObject{{"wb", wb}}}};
        aigle::Adjustments editor;
        editor.setSchema(schema);
        editor.setImageInfo(aigle::imageInfoFromJson({{"as_shot", QJsonObject{{"temperature", 6200}, {"tint", 17}}}}));
        editor.setParams(
            {{"wb", QJsonObject{{"mode", "as_shot"}, {"temperature", 6504}, {"tint", 0}}}});
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        auto* slider = editor.findChild<QSlider*>("wb.temperature");
        QVERIFY(slider);
        slider->setValue(slider->value() + 100);
        QCOMPARE(commands.size(), 1);
        const auto operations = commands.takeFirst()[0].toJsonObject().value("ops").toArray();
        QCOMPARE(operations.size(), 3);
        QCOMPARE(operations[1].toObject().value("path").toString(), QString("wb.tint"));
        QCOMPARE(operations[1].toObject().value("value").toInt(), 17);
        QCOMPARE(operations[2].toObject().value("value").toString(), QString("custom"));
    }
    void curveTranslationPreservesChannelAndPoints() {
        aigle::Adjustments editor;
        editor.setSchema({});
        auto* channel = editor.findChild<QComboBox*>("pointCurveChannel");
        auto* curve = editor.findChild<aigle::Curve*>();
        QVERIFY(channel && curve);
        channel->setCurrentIndex(2);
        const auto points = curve->points();
        const auto help = curve->toolTip();
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
        aigle::applyTranslations(*qApp, "zh-CN");
        QCOMPARE(channel->currentData().toString(), QString("green"));
        QVERIFY(channel->currentText() != "Green");
        QVERIFY(curve->toolTip() != help);
        QCOMPARE(curve->accessibleDescription(), curve->toolTip());
        QCOMPARE(curve->points(), points);
        QCOMPARE(commands.size(), 0);
    }
    void curveInsertionMatchesSharedSplineReference() {
        QFile file(QFINDTESTDATA("../../../crates/core/tests/fixtures/curve-interpolation.json"));
        QVERIFY(file.open(QIODevice::ReadOnly));
        const auto cases = QJsonDocument::fromJson(file.readAll()).array();
        QVERIFY(!cases.isEmpty());
        for (const auto& value : cases) {
            const auto fixture = value.toObject();
            for (const auto& entry : fixture["samples"].toArray()) {
                const auto sample = entry.toArray();
                aigle::Curve curve(aigle::curvePointsFromJson(fixture["points"].toArray()));
                curve.resize(320, 320);
                const double x = sample[0].toDouble();
                QTest::mouseClick(&curve, Qt::LeftButton, Qt::NoModifier,
                                  QPoint(qRound(16 + 288 * x), 80));
                bool found = false;
                for (const auto& p : aigle::toJson(curve.points())) {
                    const auto point = p.toObject();
                    if (std::abs(point["x"].toDouble() - x) > 1e-9) continue;
                    QVERIFY(std::abs(point["y"].toDouble() - sample[1].toDouble()) < 1e-9);
                    found = true;
                }
                QVERIFY(found);
            }
        }
    }
    void curveInsertionUsesCurveAndDragPreservesGrabOffset() {
        aigle::Curve curve({});
        curve.resize(320, 320);
        QTest::mousePress(&curve, Qt::LeftButton, Qt::NoModifier, {160, 80});
        QCOMPARE(aigle::toJson(curve.points()).size(), 3);
        QCOMPARE(aigle::toJson(curve.points())[1].toObject()["x"].toDouble(), .5);
        QCOMPARE(aigle::toJson(curve.points())[1].toObject()["y"].toDouble(), .5);
        QMouseEvent move(QEvent::MouseMove, QPointF(160, 64), QPointF(160, 64),
                         Qt::NoButton, Qt::LeftButton, Qt::NoModifier);
        QApplication::sendEvent(&curve, &move);
        QVERIFY(std::abs(aigle::toJson(curve.points())[1].toObject()["y"].toDouble() - (.5 + 16. / 288)) < 1e-9);
        QTest::mouseRelease(&curve, Qt::LeftButton, Qt::NoModifier, {160, 64});
    }
    void curveDragOutCanReturnBeforeDeletionCommits() {
        const QJsonArray original{QJsonObject{{"x", 0}, {"y", 0}},
            QJsonObject{{"x", .5}, {"y", .5}}, QJsonObject{{"x", 1}, {"y", 1}}};
        aigle::Curve curve(aigle::curvePointsFromJson(original));
        curve.resize(320, 320);
        QSignalSpy previews(&curve, &aigle::Curve::pointsPreviewed);
        QSignalSpy commits(&curve, &aigle::Curve::pointsChanged);
        const auto move = [&](QPointF at) {
            QMouseEvent event(QEvent::MouseMove, at, at, Qt::NoButton, Qt::LeftButton, Qt::NoModifier);
            QApplication::sendEvent(&curve, &event);
        };
        QTest::mousePress(&curve, Qt::LeftButton, Qt::NoModifier, {160, 160});
        move({160, -20});
        QCOMPARE(previews.last()[0].toJsonArray().size(), 2);
        QCOMPARE(commits.size(), 0);
        move({160, 160});
        QCOMPARE(previews.last()[0].toJsonArray(), original);
        move({160, -20});
        QTest::mouseRelease(&curve, Qt::LeftButton, Qt::NoModifier, {160, -20});
        QCOMPARE(commits.size(), 1);
        QCOMPARE(aigle::toJson(curve.points()).size(), 2);
        QTest::mousePress(&curve, Qt::LeftButton, Qt::NoModifier, {16, 304});
        move({-50, 350});
        QCOMPARE(aigle::toJson(curve.points()).size(), 2);
        QTest::mouseRelease(&curve, Qt::LeftButton, Qt::NoModifier, {-50, 350});
        QCOMPARE(aigle::toJson(curve.points()).size(), 2);
    }
    void curveDeletionProtectsEndpointsAndIgnoresEmptySpace() {
        const QJsonArray original{QJsonObject{{"x", 0}, {"y", 0}},
            QJsonObject{{"x", .5}, {"y", .5}}, QJsonObject{{"x", 1}, {"y", 1}}};
        aigle::Curve curve(aigle::curvePointsFromJson(original));
        curve.resize(320, 320);
        QSignalSpy edits(&curve, &aigle::Curve::pointsChanged);
        QTest::mouseClick(&curve, Qt::RightButton, Qt::NoModifier, {60, 60});
        QTest::mouseClick(&curve, Qt::RightButton, Qt::NoModifier, {16, 304});
        QTest::mouseClick(&curve, Qt::LeftButton, Qt::NoModifier, {5, 5});
        QCOMPARE(edits.size(), 0);
        QCOMPARE(aigle::toJson(curve.points()), original);
        QTest::mouseDClick(&curve, Qt::LeftButton, Qt::NoModifier, {160, 160});
        QCOMPARE(edits.size(), 1);
        QCOMPARE(aigle::toJson(curve.points()).size(), 2);
        QTest::mouseDClick(&curve, Qt::LeftButton, Qt::NoModifier, {16, 304});
        QCOMPARE(edits.size(), 1);
        QCOMPARE(aigle::toJson(curve.points()).size(), 2);
    }
    void curveHistogramTracksChannelAndClearsWithoutEdits() {
        aigle::Curve curve({});
        curve.resize(200, 200);
        QPalette palette;
        palette.setColor(QPalette::Base, Qt::black);
        palette.setColor(QPalette::Text, Qt::white);
        curve.setPalette(palette);
        QSignalSpy edits(&curve, &aigle::Curve::pointsChanged);
        const auto points = aigle::toJson(curve.points());
        const auto empty = curve.grab().toImage();
        curve.setHistogram(aigle::histogramFromJson({{"luma", QJsonArray{10000, 10, 10, 10000}},
                            {"red", QJsonArray{0, 0, 0, 0}}}));
        const auto luma = curve.grab().toImage();
        QVERIFY(luma != empty);
        // Clipped endpoint bins must not flatten the interior histogram.
        QVERIFY(luma.pixelColor(60, 130).red() > empty.pixelColor(60, 130).red());
        curve.setChannel("red");
        QCOMPARE(curve.grab().toImage(), empty);
        curve.setChannel("rgb");
        QCOMPARE(curve.grab().toImage(), luma);
        curve.setHistogram(aigle::histogramFromJson({}));
        QCOMPARE(curve.grab().toImage(), empty);
        QCOMPARE(aigle::toJson(curve.points()), points);
        QCOMPARE(edits.size(), 0);
    }
    void curveDragPreviewsAndCommitsOneGesture() {
        aigle::Adjustments editor;
        editor.setSchema({});
        auto* curve = editor.findChild<aigle::Curve*>();
        QVERIFY(curve);
        curve->resize(200, 200);
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        QTest::mousePress(curve, Qt::LeftButton, Qt::NoModifier, QPoint(90, 90));
        QVERIFY(!commands.isEmpty());
        const auto first = commands.first()[0].toJsonObject();
        QVERIFY(first["continuous"].toBool());
        QCOMPARE(first["ops"].toArray()[0].toObject()["path"].toString(), QString("curve.point.rgb"));
        QMouseEvent move(QEvent::MouseMove, QPointF(110, 70), QPointF(110, 70),
                         Qt::NoButton, Qt::LeftButton, Qt::NoModifier);
        QApplication::sendEvent(curve, &move);
        QVERIFY(commands.size() >= 2);
        const auto preview = commands.last()[0].toJsonObject();
        QVERIFY(preview["continuous"].toBool());
        QCOMPARE(preview["gesture"], first["gesture"]);
        QVERIFY(preview["ops"] != first["ops"]);
        QTest::mouseRelease(curve, Qt::LeftButton, Qt::NoModifier, QPoint(110, 70));
        const auto final = commands.last()[0].toJsonObject();
        QVERIFY(!final["continuous"].toBool());
        QCOMPARE(final["gesture"], first["gesture"]);
        QCOMPARE(final["ops"], preview["ops"]);
        const auto point = aigle::toJson(curve->points())[1].toObject();
        const double side = std::min(curve->width(), curve->height()) - 32;
        const QPoint grab(qRound(16 + point["x"].toDouble() * side),
                          qRound(16 + (1 - point["y"].toDouble()) * side));
        QTest::mousePress(curve, Qt::LeftButton, Qt::NoModifier, grab);
        QVERIFY(commands.last()[0].toJsonObject()["gesture"] != first["gesture"]);
        QTest::mouseRelease(curve, Qt::LeftButton, Qt::NoModifier, grab);
    }
    void rawControlsFollowSourceKindWithoutEditing() {
        aigle::Adjustments editor;
        editor.setSchema({});
        auto* decoding = editor.findChild<QWidget*>("rawDecodingControls");
        auto* dcp = editor.findChild<QPushButton*>("import.dcp");
        auto* lut = editor.findChild<QPushButton*>("import.lut");
        QVERIFY(decoding && dcp && lut);
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        for (const auto& kind : {"Raw", "Rendered", "Synthetic", "Raw"}) {
            editor.setImageInfo(aigle::imageInfoFromJson({{"kind", kind}}));
            const bool raw = QString(kind) == "Raw";
            QCOMPARE(!decoding->isHidden(), raw);
            QCOMPARE(!dcp->isHidden(), raw);
            QVERIFY(!lut->isHidden());
        }
        QCOMPARE(commands.size(), 0);
    }
    void rawSectionResetIncludesAllDecodingControls() {
        aigle::Adjustments editor;
        const QJsonObject boolean{{"type", "boolean"}, {"default", true}};
        const QJsonObject mode{{"type", "string"}, {"enum", QJsonArray{"a", "b"}}, {"default", "a"}};
        editor.setSchema({{"type", "object"}, {"properties", QJsonObject{{"raw", QJsonObject{
            {"type", "object"}, {"properties", QJsonObject{{"demosaic", mode}, {"highlight_recon", mode},
                {"ca_correction", boolean}, {"hot_pixels", boolean}}}}}}}});
        editor.setParams({{"raw", QJsonObject{{"demosaic", "b"}, {"highlight_recon", "b"},
            {"ca_correction", false}, {"hot_pixels", false}}}});
        // Enum controls belong to the same section as the section heading/reset.
        QToolButton* reset = nullptr;
        for (auto* label : editor.findChildren<QLabel*>("sectionCaption"))
            if (label->text() == "Decoding")
                for (auto* button : label->parentWidget()->findChildren<QToolButton*>())
                    if (button->toolTip() == "Reset section") reset = button;
        QVERIFY(reset);
        QVERIFY(reset->isEnabled());
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        reset->click();
        QCOMPARE(commands.size(), 1);
        const auto ops = commands.first()[0].toJsonObject()["ops"].toArray();
        QCOMPARE(ops.size(), 4);
        QStringList paths;
        for (const auto& op : ops) {
            QCOMPARE(op.toObject()["op"].toString(), QString("reset"));
            paths.append(op.toObject()["path"].toString());
        }
        paths.sort();
        QCOMPARE(paths, QStringList({"raw.ca_correction", "raw.demosaic", "raw.highlight_recon", "raw.hot_pixels"}));
    }
    void sliderReleaseKeepsOneGestureAndKeyboardStartsAnother() {
        aigle::Adjustments editor;
        const QJsonObject field{
            {"type", "number"}, {"minimum", -4}, {"maximum", 4}, {"x-step", .01}};
        editor.setSchema(
            {{"type", "object"},
             {"properties",
              QJsonObject{
                  {"light", QJsonObject{{"type", "object"},
                                        {"properties", QJsonObject{{"exposure", field}}}}}}}});
        editor.setParams({{"light", QJsonObject{{"exposure", 0}}}});
        auto* slider = editor.findChild<QSlider*>("light.exposure");
        QVERIFY(slider);
        QSignalSpy commands(&editor, &aigle::Adjustments::command);
        QDoubleSpinBox* number = nullptr;
        for (auto* candidate : editor.findChildren<QDoubleSpinBox*>())
            if (candidate->property("parameterPath") == "light.exposure") number = candidate;
        QVERIFY(number);
        QCOMPARE(number->accessibleName(), slider->accessibleName());
        const auto label = slider->accessibleName();
        const auto position = slider->value();
        const auto headers = editor.findChildren<QToolButton*>("groupHeader");
        QList<bool> expanded;
        for (auto* header : headers) expanded.append(header->isChecked());
        {
            const auto restore = qScopeGuard([] { aigle::applyTranslations(*qApp, "en"); });
            aigle::applyTranslations(*qApp, "zh-CN");
            QVERIFY(slider->accessibleName() != label);
            QCOMPARE(number->accessibleName(), slider->accessibleName());
            QCOMPARE(slider->value(), position);
            QCOMPARE(editor.findChild<QSlider*>("light.exposure"), slider);
            for (int i = 0; i < headers.size(); ++i) QCOMPARE(headers[i]->isChecked(), expanded[i]);
            QCOMPARE(commands.size(), 0);
        }
        QCOMPARE(slider->accessibleName(), label);
        slider->setSliderDown(true);
        slider->setValue(5500);
        slider->setValue(6000);
        slider->setSliderDown(false);
        QCOMPARE(commands.size(), 3);
        const auto first = commands[0][0].toJsonObject();
        const auto final = commands[2][0].toJsonObject();
        QCOMPARE(first["gesture"], final["gesture"]);
        QVERIFY(first["continuous"].toBool());
        QVERIFY(!final["continuous"].toBool());
        slider->setValue(6100);
        QVERIFY(commands.last()[0].toJsonObject()["gesture"] != final["gesture"]);
    }
    void rangeMaskCreatesNormalizedCoverageWithoutChangingExistingMasks() {
        aigle::ToolPanel editor;
        const QJsonObject original{
            {"id", 7},
            {"name", "Sky"},
            {"opacity", .7},
            {"shape",
             QJsonObject{{"kind", "range"},
                         {"range", QJsonObject{{"kind", "luminance"}, {"low", .1}, {"high", .8}}},
                         {"smoothness", 10}}}};
        editor.setParams({{"masks", QJsonArray{original}}});
        editor.setPage("masks");
        QSignalSpy commands(&editor, &aigle::ToolPanel::command);
        QPushButton* add = nullptr;
        for (auto* button : editor.findChildren<QPushButton*>())
            if (button->text() == "+ Range mask")
                add = button;
        QVERIFY(add);
        QTimer::singleShot(0, &editor, [] {
            auto* dialog = qobject_cast<QDialog*>(QApplication::activeModalWidget());
            QVERIFY(dialog);
            auto numbers = dialog->findChildren<QDoubleSpinBox*>();
            QCOMPARE(numbers.size(), 3);
            numbers[0]->setValue(.8);
            numbers[1]->setValue(.2);
            dialog->accept();
        });
        add->click();
        QCOMPARE(commands.size(), 1);
        const auto masks =
            commands[0][0].toJsonObject()["ops"].toArray()[0].toObject()["value"].toArray();
        QCOMPARE(masks.size(), 2);
        QCOMPARE(masks[0].toObject(), original);
        QCOMPARE(masks[1].toObject()["id"].toInt(), 8);
        const auto range = masks[1].toObject()["shape"].toObject()["range"].toObject();
        QCOMPARE(range["low"].toDouble(), .2);
        QCOMPARE(range["high"].toDouble(), .8);
    }
    void inspectorSearchRevealsControlsWithoutActivatingTools() {
        aigle::Adjustments editor;
        editor.setSchema({});
        editor.show();
        auto* tabs = editor.findChild<QTabWidget*>("developTabs");
        QVERIFY(tabs);
        QVERIFY(!tabs->tabBar()->isVisible());
        QSignalSpy tools(&editor, &aigle::Adjustments::toolSelected);
        auto* search = editor.findChild<QLineEdit*>("adjustSearch");
        QVERIFY(search);
        search->setText("Curves");
        auto groups = editor.findChildren<QWidget*>("adjustmentGroup");
        int visible = 0;
        for (auto* group : groups)
            if (group->isVisible()) {
                ++visible;
                auto* header = group->findChild<QToolButton*>("groupHeader");
                QCOMPARE(header->text(), QString("Curves"));
                QVERIFY(header->isChecked());
            }
        QCOMPARE(visible, 1);
        QCOMPARE(tools.size(), 0);
        search->clear();
        QCOMPARE(groups.size(), 8);
    }
    void colorRangeControlsKeepNestedShapeAndSamples() {
        aigle::ToolPanel editor;
        const QJsonArray samples{QJsonArray{.1, .2, .3}};
        const QJsonObject range{{"kind", "color"}, {"samples", samples}, {"tolerance", 20}};
        const QJsonObject initialShape{{"kind", "range"}, {"smoothness", 20}, {"range", range}};
        const QJsonObject initialMask{{"id", 9}, {"name", "Color"}, {"opacity", 1.0},
            {"adjust", QJsonObject{}}, {"shape", initialShape}};
        editor.setParams(QJsonObject{{"masks", QJsonArray{initialMask}}});
        editor.setPage("masks");
        QSignalSpy commands(&editor, &aigle::ToolPanel::command);
        QDoubleSpinBox* tolerance = nullptr;
        for (auto* spin : editor.findChildren<QDoubleSpinBox*>())
            if (spin->accessibleName() == "Color tolerance") tolerance = spin;
        QVERIFY(tolerance);
        tolerance->setValue(35);
        QCOMPARE(commands.size(), 1);
        const auto mask = commands[0][0].toJsonObject()["ops"].toArray()[0].toObject()["value"].toArray()[0].toObject();
        const auto shape = mask["shape"].toObject();
        QVERIFY(!shape.contains("range.tolerance"));
        QCOMPARE(shape["range"].toObject()["tolerance"].toInt(), 35);
        QCOMPARE(shape["range"].toObject()["samples"].toArray(), samples);
        QCOMPARE(shape["smoothness"].toInt(), 20);
    }

    void maskEditsPreserveIdentityAndUnrelatedAdjustments() {
        aigle::ToolPanel editor;
        editor.setParams(
            {{"masks", QJsonArray{QJsonObject{
                           {"id", 7},
                           {"name", "Sky"},
                           {"invert", false},
                           {"opacity", 1.0},
                           {"shape", QJsonObject{{"kind", "linear"},
                                                 {"from", QJsonObject{{"x", .2}, {"y", .2}}},
                                                 {"to", QJsonObject{{"x", .2}, {"y", .7}}}}},
                           {"adjust", QJsonObject{{"exposure", .5}, {"contrast", 12}}}}}}});
        editor.setPage("masks");
        QSignalSpy commands(&editor, &aigle::ToolPanel::command);
        auto spins = editor.findChildren<QDoubleSpinBox*>();
        QDoubleSpinBox* exposure = nullptr;
        for (auto* spin : spins)
            if (spin->accessibleName() == "Exposure")
                exposure = spin;
        QVERIFY(exposure);
        exposure->setValue(.8);
        QCOMPARE(commands.size(), 1);
        auto mask = commands.first()[0]
                        .toJsonObject()["ops"]
                        .toArray()[0]
                        .toObject()["value"]
                        .toArray()[0]
                        .toObject();
        QCOMPARE(mask["id"].toInt(), 7);
        QCOMPARE(mask["adjust"].toObject()["contrast"].toInt(), 12);
        QCOMPARE(mask["adjust"].toObject()["exposure"].toDouble(), .8);
    }
    void exportPreviewFailureIsVisibleWithoutBlockingExport() {
        const QJsonArray formats{QJsonObject{{"name", "PNG"}, {"extension", "png"}, {"format", "Png"}}};
        const QJsonArray inputs{QJsonObject{{"name", "photo.png"}, {"raw", false},
            {"preview_error", "Cannot decode preview"}}};
        aigle::ExportDialog dialog("/tmp/photo.png", {}, {100,100},aigle::exportFormatsFromJson( formats), nullptr,aigle::exportInputsFromJson( inputs));
        auto* preview = dialog.findChild<QLabel*>("photoPreview");
        QVERIFY(preview);
        QCOMPARE(preview->text(), QString("Preview unavailable"));
        QCOMPARE(preview->toolTip(), QString("Cannot decode preview"));
        auto* format = dialog.findChild<QComboBox*>("exportFormat");
        format->setCurrentIndex(format->findText("PNG"));
        QCOMPARE(exportRequestsJson(dialog.requests()).size(), 1);
        QVERIFY(dialog.findChild<QPushButton*>("primary")->isEnabled());
        QImage edited(80,60,QImage::Format_ARGB32);
        edited.fill(Qt::red);
        aigle::ExportDialog available("/tmp/photo.png", edited, {100,100},aigle::exportFormatsFromJson( formats));
        auto* rendered = available.findChild<QLabel*>("photoPreview");
        QVERIFY(!rendered->pixmap().isNull());
        QVERIFY(rendered->toolTip().contains("watermarks are not shown"));
    }
    void unavailableExportFormatsRequireExplicitChoice() {
        QTemporaryDir dir;
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, dir.path());
        QSettings().setValue("export/lastSettings", QJsonDocument(QJsonObject{{"format", "DNG"}}).toJson());
        const QJsonArray formats{QJsonObject{{"name", "PNG"}, {"extension", "png"}, {"format", "Png"}},
            QJsonObject{{"name", "DNG"}, {"extension", "dng"}, {"format", "Dng"}}};
        const QJsonObject raw{{"name", "raw.NEF"}, {"raw", true}}, raster{{"name", "image.png"}, {"raw", false}};
        aigle::ExportDialog mixed("/tmp/raw.NEF", {}, {100,100},aigle::exportFormatsFromJson( formats), nullptr,aigle::exportInputsFromJson( QJsonArray{raw, raster}));
        auto* format = mixed.findChild<QComboBox*>("exportFormat");
        QCOMPARE(format->findText("DNG"), -1);
        QCOMPARE(format->currentIndex(), -1);
        QVERIFY(exportRequestsJson(mixed.requests()).isEmpty());
        QVERIFY(!mixed.findChild<QPushButton*>("primary")->isEnabled());
        format->setCurrentIndex(format->findText("PNG"));
        QCOMPARE(exportRequestsJson(mixed.requests()).size(), 2);
        QVERIFY(mixed.findChild<QPushButton*>("primary")->isEnabled());
        aigle::ExportDialog rawOnly("/tmp/raw.NEF", {}, {100,100},aigle::exportFormatsFromJson( formats), nullptr,aigle::exportInputsFromJson( QJsonArray{raw}));
        QCOMPARE(rawOnly.findChild<QComboBox*>("exportFormat")->currentText(), QString("DNG"));
        QVERIFY(!rawOnly.findChild<QComboBox*>("exportWatermarkKind")->isEnabled());
        QVERIFY(exportRequestsJson(rawOnly.requests())[0].toObject()["options"].toObject()["long_edge"].isNull());
        const QJsonObject dng{{"name", "copy"}, {"raw", true},
            {"source_ref", QJsonObject{{"File", "/tmp/source.DNG"}}}};
        aigle::ExportDialog dngBatch("/tmp/raw.NEF", {}, {100,100},aigle::exportFormatsFromJson( formats), nullptr,aigle::exportInputsFromJson( QJsonArray{raw,dng}));
        QCOMPARE(dngBatch.findChild<QComboBox*>("exportFormat")->findText("DNG"), -1);
        QVERIFY(exportRequestsJson(dngBatch.requests()).isEmpty());
        QVERIFY(!dngBatch.findChild<QPushButton*>("primary")->isEnabled());
        aigle::ExportDialog dngSingle("/tmp/source.dng", {}, {100,100},aigle::exportFormatsFromJson( formats));
        QCOMPARE(dngSingle.findChild<QComboBox*>("exportFormat")->findText("DNG"), -1);
        QSettings().setValue("export/lastSettings", QJsonDocument(QJsonObject{{"format", "Unavailable codec"}}).toJson());
        aigle::ExportDialog unavailable("/tmp/raw.NEF", {}, {100,100},aigle::exportFormatsFromJson( formats));
        QVERIFY(exportRequestsJson(unavailable.requests()).isEmpty());
        QVERIFY(!unavailable.findChild<QPushButton*>("primary")->isEnabled());
        QSettings().remove("export/lastSettings");
    }
    void batchExportNamingAndSnapshots() {
        QJsonArray inputs;
        for (int i = 0; i < 12; ++i) inputs.append(QJsonObject{{"name", "Same.NEF"},
            {"source_ref", QJsonObject{{"File", QString("/source/%1.NEF").arg(i)}}},
            {"params", QJsonObject{{"value", i}}}});
        const QJsonArray formats{QJsonObject{{"name", "PNG"}, {"extension", "png"}, {"format", "Png"}}};
        aigle::ExportDialog dialog("/source/Same.NEF", {}, {100,100},aigle::exportFormatsFromJson( formats), nullptr,aigle::exportInputsFromJson( inputs));
        auto* naming = dialog.findChild<QLineEdit*>("exportNaming");
        naming->setText("{seq}-{name}");
        auto jobs = exportRequestsJson(dialog.requests());
        QCOMPARE(jobs.size(), 12);
        QVERIFY(jobs[0].toObject()["options"].toObject()["path"].toString().endsWith("01-Same.png"));
        QVERIFY(jobs[11].toObject()["options"].toObject()["path"].toString().endsWith("12-Same.png"));
        naming->setText("../same");
        jobs = exportRequestsJson(dialog.requests());
        QSet<QString> paths;
        for (int i = 0; i < jobs.size(); ++i) {
            const auto job = jobs[i].toObject();
            const auto path = job["options"].toObject()["path"].toString();
            QVERIFY(!paths.contains(path)); paths.insert(path);
            QVERIFY(QFileInfo(path).fileName().startsWith(".._same"));
            QCOMPARE(job["params"].toObject()["value"].toInt(), i);
        }
    }
    void exportRestoresNamedPresetAndSerializesFinishingOptions() {
        QSettings store;
        const auto previous = store.value("export/presets");
        const auto restore = qScopeGuard([&] { if (previous.isValid()) store.setValue("export/presets", previous); else store.remove("export/presets"); });
        const QJsonObject preset{{"format", "JPEG"}, {"longEdge", 1379}, {"size", 0},
            {"name", "{name}-custom"}, {"artist", "Photographer"}, {"copyright", "Copyright"},
            {"watermark", 2}, {"image", "/tmp/logo.png"}, {"wmSize", 12.5}, {"opacity", 42},
            {"margin", 7}, {"anchor", 0}, {"color", "#123456"}};
        store.setValue("export/presets", QJsonDocument(QJsonObject{{"Custom delivery", preset}}).toJson());
        const QJsonArray formats{QJsonObject{{"name", "JPEG"}, {"extension", "jpg"},
            {"format", QJsonObject{{"Jpeg", QJsonObject{{"quality", 90}}}}}}};
        aigle::ExportDialog dialog("/tmp/example.NEF", QImage(), QSize(6000, 4000),aigle::exportFormatsFromJson( formats));
        bool selected = false;
        for (auto* combo : dialog.findChildren<QComboBox*>()) {
            const auto index = combo->findText("Custom delivery");
            if (index < 0) continue;
            combo->setCurrentIndex(index);
            QVERIFY(QMetaObject::invokeMethod(combo, "activated", Q_ARG(int, index)));
            selected = true;
        }
        QVERIFY(selected);
        auto options = exportRequestsJson(dialog.requests())[0].toObject()["options"].toObject();
        QCOMPARE(options["long_edge"].toInt(), 1379);
        QCOMPARE(options["artist"].toString(), QString("Photographer"));
        QCOMPARE(options["copyright"].toString(), QString("Copyright"));
        const auto watermark = options["watermark"].toObject();
        QCOMPARE(watermark["content"].toObject()["Image"].toString(), QString("/tmp/logo.png"));
        QCOMPARE(watermark["anchor"].toString(), QString("TopLeft"));
        QCOMPARE(watermark["opacity"].toDouble(), .42);
        QCOMPARE(watermark["size"].toDouble(), .125);
        QCOMPARE(watermark["color"].toArray(), QJsonArray({18, 52, 86}));
    }

    void exportKeepsHdrEncodingAndPairedSdrSeparate() {
        const QJsonArray formats{
            QJsonObject{{"name", "JPEG"},
                        {"extension", "jpg"},
                        {"format", QJsonObject{{"Jpeg", QJsonObject{{"quality", 90}}}}}},
            QJsonObject{{"name", "HDR PNG · PQ"}, {"extension", "png"}, {"format", "HdrPng"}}};
        aigle::ExportDialog dialog("/tmp/example.NEF", QImage(), QSize(6000, 4000),aigle::exportFormatsFromJson( formats));
        for (auto* combo : dialog.findChildren<QComboBox*>())
            if (combo->findText("HDR + SDR delivery") >= 0) {
                combo->setCurrentIndex(2);
                QVERIFY(QMetaObject::invokeMethod(combo, "activated", Q_ARG(int, 2)));
                break;
            }
        const auto jobs = exportRequestsJson(dialog.requests());
        QCOMPARE(jobs.size(), 2);
        QCOMPARE(jobs[0].toObject()["options"].toObject()["format"].toString(), QString("HdrPng"));
        const auto sdr = jobs[1].toObject()["options"].toObject();
        QVERIFY(sdr["path"].toString().endsWith("-sdr.jpg"));
        QVERIFY(sdr["format"].toObject().contains("Jpeg"));
        QCOMPARE(sdr["output"].toString(), QString("Srgb"));
        QVERIFY(!jobs[0].toObject()["overwrite"].toBool());
    }

    void optionalMaskAdjustmentKeepsItsType() {
        const QJsonObject schema{
            {"type", QJsonArray{"number", "null"}}, {"minimum", -4}, {"maximum", 4}};
        aigle::ValueEditor editor(schema, QJsonValue::Null, schema);
        QVERIFY(editor.value().isNull());
        auto* choice = editor.findChild<QComboBox*>();
        QVERIFY(choice);
        QVERIFY(QMetaObject::invokeMethod(choice, "activated", Q_ARG(int, 0)));
        auto* spin = editor.findChild<QDoubleSpinBox*>();
        QVERIFY(spin);
        spin->setValue(1.25);
        QCOMPARE(editor.value().toDouble(), 1.25);
    }
    void nullableObjectCanBeDisabled() {
        const QJsonObject schema{
            {"type", QJsonArray{"object", "null"}},
            {"properties", QJsonObject{{"amount", QJsonObject{{"type", "number"}}}}}};
        aigle::ValueEditor editor(schema, QJsonObject{{"amount", 0.5}}, schema);
        QVERIFY(editor.value().isObject());
        auto* choice = editor.findChild<QComboBox*>();
        QVERIFY(choice);
        QVERIFY(QMetaObject::invokeMethod(choice, "activated", Q_ARG(int, 1)));
        QVERIFY(editor.value().isNull());
    }
};
QTEST_MAIN(ValueEditorTest)
#include "value_editor_test.moc"
