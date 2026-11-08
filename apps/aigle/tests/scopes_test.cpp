#include "gui_json.hpp"
#include "command_json.hpp"
#include "scopes.hpp"
#include <QJsonArray>
#include <QKeyEvent>
#include <QPushButton>
#include <QSettings>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QToolButton>
using namespace aigle;
class ScopesTest final : public QObject {
    Q_OBJECT
    QTemporaryDir storage_;
    QJsonObject reply(const QString& id, bool hdr = false) const {
        QJsonArray bins, wave;
        for (int c = 0; c < 4; ++c) {
            QJsonArray values, density;
            for (int i = 0; i < 256; ++i)
                values.append(i == 64 ? 2 : i == 128 ? 3 : i == 192 ? 3 : 0);
            for (int i = 0; i < 4096; ++i)
                density.append(i == (16 + c * 8) * 64 + 32 ? 8 : 0);
            bins.append(values);
            wave.append(density);
        }
        QJsonArray vector, thumbnail;
        for (int i = 0; i < 4096; ++i)
            vector.append(i == 32 * 64 + 32 ? 8 : 0);
        for (int i = 0; i < 8; ++i)
            thumbnail.append(QJsonArray{100 + i * 10, 120, 140, 255});
        return {{"request_id", id},
                {"available", true},
                {"width", 4},
                {"height", 2},
                {"thumbnail", thumbnail},
                {"diffuse_white", 0.58},
                {"targets", QJsonArray{QJsonArray{-0.1, 0.5}, QJsonArray{0.5, -0.1}}},
                {"data", QJsonObject{{"bins", bins},
                                     {"wave", wave},
                                     {"vector", vector},
                                     {"count", 8},
                                     {"low", 2},
                                     {"high", 1},
                                     {"gamut", 3},
                                     {"hdr", hdr}}}};
    }
    QPoint samplePoint(ScopeThumbnail& thumbnail, int x, int y) const {
        const auto area = thumbnail.imageRect();
        return QPoint(int(area.left() + (x + 0.5) * area.width() / 4),
                      int(area.top() + (y + 0.5) * area.height() / 2));
    }
  private slots:
    void initTestCase() {
        QMetaType::registerConverter<aigle::EngineCommand, QJsonObject>([](const aigle::EngineCommand& command) { return aigle::toJson(command); });
        QVERIFY(storage_.isValid());
        QSettings::setDefaultFormat(QSettings::IniFormat);
        QSettings::setPath(QSettings::IniFormat, QSettings::UserScope, storage_.path());
    }
    void init() { QSettings().clear(); }
    void modesUseCachedDistributionsAndPersistPreferences() {
        Scopes scopes;
        scopes.resize(240, 520);
        scopes.show();
        QSignalSpy commands(&scopes, &Scopes::command);
        scopes.histogramChanged(aigle::histogramFromJson({{"red", QJsonArray{1}}}));
        QTRY_COMPARE(commands.count(), 1);
        const auto id = commands.last().at(0).toJsonObject().value("request_id").toString();
        scopes.setResult(aigle::scopeResultFromJson(reply(id)));
        auto* stats = scopes.findChild<QLabel*>("scopeStatistics");
        QVERIFY(stats->text().contains("25.0%"));
        QVERIFY(stats->text().contains("12.5%"));
        QVERIFY(stats->text().contains("37.5%"));
        auto* mode = scopes.findChild<QComboBox*>("scopeMode");
        QCOMPARE(mode->count(), 7);
        auto* graph = static_cast<ScopeGraph*>(scopes.findChild<QWidget*>("scopeGraph"));
        QVector<QImage> images;
        for (int i = 0; i < 7; ++i) {
            mode->setCurrentIndex(i);
            images.append(graph->grab().toImage());
        }
        QVERIFY(images.at(0) != images.at(4));
        QVERIFY(images.at(4) != images.at(5));
        QVERIFY(images.at(5) != images.at(6));
        QTest::mouseMove(graph, graph->plotRect().center().toPoint());
        QVERIFY(scopes.findChild<QLabel*>("scopeReadout")->text().contains("Cb"));
        mode->setCurrentIndex(3);
        QTest::mouseMove(graph, (graph->plotRect().center() + QPointF(1, 0)).toPoint());
        QVERIFY(scopes.findChild<QLabel*>("scopeReadout")->text().contains("% below"));
        scopes.findChild<QCheckBox*>("scopeLog")->setChecked(true);
        QTest::qWait(90);
        QCOMPARE(commands.count(),
                 1); // Hover, scaling and modes do not invoke analysis or renders.
        Scopes reopened;
        QCOMPARE(reopened.findChild<QComboBox*>("scopeMode")->currentIndex(), 3);
        QVERIFY(reopened.findChild<QCheckBox*>("scopeLog")->isChecked());
    }
    void regionClickProducesOneCellAndResetUsesExactWholeCrop() {
        Scopes scopes;
        scopes.resize(240, 520);
        scopes.show();
        QSignalSpy commands(&scopes, &Scopes::command);
        scopes.histogramChanged(aigle::histogramFromJson({{"red", QJsonArray{1}}}));
        QTRY_COMPARE(commands.count(), 1);
        scopes.setResult(aigle::scopeResultFromJson(
            reply(commands.last().at(0).toJsonObject().value("request_id").toString())));
        scopes.findChild<QCheckBox*>("scopeRegion")->setChecked(true);
        QTRY_COMPARE(commands.count(), 2);
        scopes.setResult(aigle::scopeResultFromJson(
            reply(commands.last().at(0).toJsonObject().value("request_id").toString())));
        auto* thumbnail =
            static_cast<ScopeThumbnail*>(scopes.findChild<QWidget*>("scopeRegionImage"));
        QTest::mouseClick(thumbnail, Qt::LeftButton, Qt::NoModifier, samplePoint(*thumbnail, 0, 0));
        QTRY_COMPARE(commands.count(), 3);
        QCOMPARE(commands.last().at(0).toJsonObject().value("roi").toArray(),
                 (QJsonArray{0.0, 0.0, 0.25, 0.5}));
        QCOMPARE(commands.last().at(0).toJsonObject().value("cmd").toString(), QString("scopes"));
        auto resized = reply(commands.last().at(0).toJsonObject().value("request_id").toString());
        resized["width"] = 8;
        resized["height"] = 4;
        scopes.setResult(aigle::scopeResultFromJson(resized));
        QTRY_COMPARE(commands.count(), 4);
        QVERIFY(commands.last().at(0).toJsonObject().value("roi").isNull());
        scopes.setResult(aigle::scopeResultFromJson(
            reply(commands.last().at(0).toJsonObject().value("request_id").toString())));
        scopes.findChild<QPushButton*>("scopeWholeCrop")->click();
        QTRY_COMPARE(commands.count(), 5);
        QVERIFY(commands.last().at(0).toJsonObject().value("roi").isNull());
    }
    void thumbnailKeyboardAndCancelledDragPreserveTheSelection() {
        ScopeThumbnail thumbnail;
        thumbnail.resize(200, 80);
        thumbnail.show();
        QImage image(4, 2, QImage::Format_RGBA8888);
        image.fill(Qt::white);
        thumbnail.setSample(image);
        QRect last;
        thumbnail.selected = [&](const QRect& selected) { last = selected; };
        QTest::mouseClick(&thumbnail, Qt::LeftButton, Qt::NoModifier, samplePoint(thumbnail, 0, 0));
        QCOMPARE(last, QRect(0, 0, 1, 1));
        QTest::keyClick(&thumbnail, Qt::Key_Right, Qt::ShiftModifier);
        QCOMPARE(last, QRect(0, 0, 2, 1));
        QTest::keyClick(&thumbnail, Qt::Key_Down);
        QCOMPARE(last, QRect(0, 1, 2, 1));
        const auto saved = last;
        QTest::mousePress(&thumbnail, Qt::LeftButton, Qt::NoModifier, samplePoint(thumbnail, 3, 0));
        QTest::mouseMove(&thumbnail, samplePoint(thumbnail, 1, 1));
        QTest::keyClick(&thumbnail, Qt::Key_Escape);
        QTest::mouseRelease(&thumbnail, Qt::LeftButton, Qt::NoModifier,
                            samplePoint(thumbnail, 1, 1));
        QCOMPARE(last, saved);
        QTest::keyClick(&thumbnail, Qt::Key_Home);
        QVERIFY(last.isEmpty());
        QKeyEvent shortcut(QEvent::ShortcutOverride, Qt::Key_Left, Qt::NoModifier);
        shortcut.ignore();
        QApplication::sendEvent(&thumbnail, &shortcut);
        QVERIFY(shortcut.isAccepted());
        QTest::mousePress(&thumbnail, Qt::LeftButton, Qt::NoModifier, samplePoint(thumbnail, 1, 1));
        thumbnail.setSample({});
        QTest::mouseRelease(&thumbnail, Qt::LeftButton, Qt::NoModifier, QPoint(0, 0));
        QVERIFY(thumbnail.selection().isEmpty());
    }
    void obsoleteRegionAndPhotoRepliesCannotReplaceCurrentData() {
        Scopes scopes;
        QSignalSpy commands(&scopes, &Scopes::command);
        scopes.histogramChanged(aigle::histogramFromJson({{"red", QJsonArray{1}}}));
        QTRY_COMPARE(commands.count(), 1);
        const auto first = commands.last().at(0).toJsonObject().value("request_id").toString();
        scopes.findChild<QComboBox*>("scopeGamutTarget")->setCurrentIndex(2);
        scopes.setResult(aigle::scopeResultFromJson(reply(first)));
        QVERIFY(scopes.findChild<QLabel*>("scopeStatistics")->text().contains("Updating"));
        QTRY_COMPARE(commands.count(), 2);
        const auto second = commands.last().at(0).toJsonObject().value("request_id").toString();
        QVERIFY(first != second);
        QCOMPARE(commands.last().at(0).toJsonObject().value("target").toInt(), 2);
        scopes.setResult(aigle::scopeResultFromJson(reply(second, true)));
        auto* graph = scopes.findChild<QWidget*>("scopeGraph");
        QTest::keyClick(graph, Qt::Key_Home);
        QVERIFY(scopes.findChild<QLabel*>("scopeReadout")->text().contains("0.0 cd/m²"));
        QTest::keyClick(graph, Qt::Key_End);
        QVERIFY(scopes.findChild<QLabel*>("scopeReadout")->text().contains("1000.0 cd/m²"));
        QKeyEvent shortcut(QEvent::ShortcutOverride, Qt::Key_Right, Qt::NoModifier);
        shortcut.ignore();
        QApplication::sendEvent(graph, &shortcut);
        QVERIFY(shortcut.isAccepted());
        QVERIFY(scopes.findChild<QLabel*>("scopeStatistics")->text().contains("8 samples"));
        scopes.reset();
        scopes.setResult(aigle::scopeResultFromJson(reply(first)));
        QVERIFY(scopes.findChild<QLabel*>("scopeStatistics")->text().contains("Waiting"));
        scopes.histogramChanged(aigle::histogramFromJson({{"red", QJsonArray{1}}}));
        QTRY_COMPARE(commands.count(), 3);
        const auto third = commands.last().at(0).toJsonObject().value("request_id").toString();
        scopes.setResult(aigle::scopeResultFromJson({{"request_id", third}, {"error", "No analysis available"}}));
        QVERIFY(scopes.findChild<QLabel*>("scopeStatistics")->text().contains("Analysis failed"));
        scopes.histogramChanged(aigle::histogramFromJson({{"red", QJsonArray{1}}}));
        QTRY_COMPARE(commands.count(), 4);
    }
};
QTEST_MAIN(ScopesTest)
#include "scopes_test.moc"
