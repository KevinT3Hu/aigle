#include "export_queue.hpp"
#include "preferences.hpp"
#include <QTemporaryDir>
#include <QTest>
using namespace aigle;
class QueueTest final : public QObject {
    Q_OBJECT
    commands::Export request(const QString& output) {
        return {.options = ExportOptions{.path = output},
                .source_ref = QJsonObject{{"File", "/photo.nef"}},
                .params = QJsonObject{{"exposure", 0.75}}};
    }
  private slots:
    void preferencesPersistAndClampStoredRanges() {
        QTemporaryDir dir;
        const auto path=dir.filePath("preferences.ini");
        { QSettings settings(path,QSettings::IniFormat);
          Preferences preferences{true,true,320,2500};
          QVERIFY(preferences.save(settings)); }
        QSettings settings(path,QSettings::IniFormat);
        const auto restored=Preferences::load(settings);
        QVERIFY(restored.recursive);QVERIFY(restored.hdr);
        QCOMPARE(restored.thumbnail,320);QCOMPARE(restored.autosaveMs,2500);
        settings.setValue("preferences/thumbnail",9000);
        settings.setValue("preferences/autosaveMs",-1);
        const auto clamped=Preferences::load(settings);
        QCOMPARE(clamped.thumbnail,340);QCOMPARE(clamped.autosaveMs,500);
    }

    void recoveryKeepsSnapshotsAndNeverAutostarts() {
        QTemporaryDir dir;
        const auto path = dir.filePath("queue.json");
        ExportQueue first(path);
        QVERIFY(first.add({request("one.png"), request("two.png")}));
        first.paused = false;
        const auto started = first.next();
        QVERIFY(started.has_value());
        QVERIFY(!first.next());
        first = ExportQueue(); // Process exits and releases its lock.
        ExportQueue second(path);
        QVERIFY(second.load());
        QVERIFY(second.paused);
        QVERIFY(!second.next());
        QCOMPARE(second.jobs[0].status, ExportStatus::Interrupted);
        QCOMPARE(second.jobs[1].status, ExportStatus::Pending);
        QCOMPARE(second.jobs[0].request, request("one.png"));
        second.paused = false;
        QCOMPARE(second.next()->options->path, QString("two.png"));
    }
    void pauseRetryCancellationAndStaleReplies() {
        QTemporaryDir dir;
        ExportQueue queue(dir.filePath("queue.json"));
        QVERIFY(queue.add({request("one.png"), request("two.png")}));
        queue.paused = false;
        const auto first = queue.next();
        const auto id = *first->job_id;
        queue.paused = true;
        QVERIFY(queue.finish({.jobId = id, .error = "Destination exists"}));
        QVERIFY(!queue.next());
        QVERIFY(queue.retry(id, "new.png"));
        queue.paused = false;
        const auto retry = queue.next();
        QVERIFY(*retry->job_id != id);
        QCOMPARE(retry->options->path, QString("new.png"));
        QCOMPARE(retry->overwrite, std::optional<bool>(false));
        QVERIFY(!queue.finish({.jobId = id, .error = "Stale failure"}));
        QVERIFY(queue.cancelPending());
        QCOMPARE(queue.jobs[1].status, ExportStatus::Cancelled);
        QVERIFY(queue.finish({.jobId = *retry->job_id, .cancelled = true}));
        QVERIFY(!queue.hasPending());
        QVERIFY(queue.clearFinished());
        QVERIFY(queue.jobs.isEmpty());
    }
    void concurrentWritersAreRejected() {
        QTemporaryDir dir;
        const auto path = dir.filePath("queue.json");
        ExportQueue first(path), second(path);
        QVERIFY(first.load());
        QVERIFY(!second.load());
        QVERIFY(first.add({request("one.png")}));
        QVERIFY(!second.add({request("two.png")}));
        QVERIFY(second.jobs.isEmpty());
    }
    void corruptStorageAndWriteFailureDoNotDispatch() {
        QTemporaryDir dir;
        const auto path = dir.filePath("queue.json");
        QFile corrupt(path);
        QVERIFY(corrupt.open(QIODevice::WriteOnly));
        corrupt.write("not JSON");
        corrupt.close();
        ExportQueue queue(path);
        QVERIFY(!queue.load());
        QVERIFY(!queue.add({request("out.png")}));
        QVERIFY(queue.jobs.isEmpty());
        queue.paused = false;
        QVERIFY(!queue.next());
        QVERIFY(corrupt.open(QIODevice::ReadOnly));
        QCOMPARE(corrupt.readAll(), QByteArray("not JSON"));
        ExportQueue invalid(dir.path()); // Target is a directory, not a file.
        QVERIFY(!invalid.add({request("out.png")}));
        QVERIFY(invalid.jobs.isEmpty());
    }
};
QTEST_GUILESS_MAIN(QueueTest)
#include "export_queue_test.moc"
