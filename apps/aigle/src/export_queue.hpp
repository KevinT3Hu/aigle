#pragma once
#include "command_json.hpp"
#include "export_types.hpp"
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QLockFile>
#include <QSaveFile>
#include <QSet>
#include <QUuid>
#include <memory>

namespace aigle {
enum class ExportStatus { Pending, Running, Done, Failed, Cancelled, Interrupted };
inline QString exportStatusName(ExportStatus status) {
    switch (status) {
    case ExportStatus::Pending: return "pending";
    case ExportStatus::Running: return "running";
    case ExportStatus::Done: return "done";
    case ExportStatus::Failed: return "failed";
    case ExportStatus::Cancelled: return "cancelled";
    case ExportStatus::Interrupted: return "interrupted";
    }
    return {};
}
struct ExportJob {
    QString id;
    commands::Export request;
    ExportStatus status = ExportStatus::Pending;
    std::optional<QString> error;
    bool operator==(const ExportJob&) const = default;
};
// JSON exists only while loading/saving the durable queue file.
class ExportQueue {
  public:
    QList<ExportJob> jobs;
    bool paused = true;
    QString error;
    explicit ExportQueue(QString path = {}) : path_(std::move(path)) {}
    bool load() {
        if (!lock()) return false;
        QFile file(path_);
        if (!file.exists()) return true;
        if (!file.open(QIODevice::ReadOnly)) return fail(file.errorString());
        QJsonParseError parse;
        const auto doc = QJsonDocument::fromJson(file.readAll(), &parse);
        if (parse.error != QJsonParseError::NoError || !doc.isObject() ||
            doc.object()["version"].toInt() != 1 || !doc.object()["jobs"].isArray())
            return fail("Cannot read export queue; the original file has been preserved.");
        QList<ExportJob> recovered;
        QSet<QString> ids;
        for (const auto& value : doc.object()["jobs"].toArray()) {
            const auto object = value.toObject();
            ExportJob job;
            job.id = object["id"].toString();
            std::optional<ExportStatus> status;
            for (const auto candidate : {ExportStatus::Pending, ExportStatus::Running, ExportStatus::Done,
                    ExportStatus::Failed, ExportStatus::Cancelled, ExportStatus::Interrupted})
                if (object["status"] == exportStatusName(candidate)) status = candidate;
            if (job.id.isEmpty() || ids.contains(job.id) || !object["request"].isObject() || !status)
                return fail("Invalid export queue; the original file has been preserved.");
            ids.insert(job.id);
            auto request = object["request"].toObject();
            request["cmd"] = "export";
            job.request = std::get<commands::Export>(*commandFromJson(request));
            job.status = *status;
            if (object.contains("error")) job.error = object["error"].toString();
            if (job.status == ExportStatus::Running) {
                job.status = ExportStatus::Interrupted;
                job.error = "Interrupted by shutdown. Check the destination before retrying.";
            }
            recovered.append(job);
        }
        jobs = recovered;
        paused = true;
        return true;
    }
    bool save() {
        if (!lock() || !error.isEmpty()) return false;
        if (!QDir().mkpath(QFileInfo(path_).absolutePath()))
            return fail("Cannot create export queue directory.");
        QJsonArray encoded;
        for (const auto& job : jobs) {
            QJsonObject object{{"id", job.id}, {"request", toJson(job.request)}, {"status", exportStatusName(job.status)}};
            if (job.error) object["error"] = *job.error;
            encoded.append(object);
        }
        QSaveFile file(path_);
        const auto bytes = QJsonDocument(QJsonObject{{"version", 1}, {"jobs", encoded}}).toJson();
        if (!file.open(QIODevice::WriteOnly) || file.write(bytes) != bytes.size() || !file.commit())
            return fail(file.errorString());
        return true;
    }
    bool add(const QList<commands::Export>& requests) {
        const auto before = jobs;
        for (const auto& request : requests)
            jobs.append({QUuid::createUuid().toString(QUuid::WithoutBraces), request, ExportStatus::Pending, {}});
        if (save()) return true;
        jobs = before;
        return false;
    }
    std::optional<commands::Export> next() {
        if (paused || !error.isEmpty() || !runningId().isEmpty()) return {};
        for (auto& job : jobs) {
            if (job.status != ExportStatus::Pending) continue;
            job.status = ExportStatus::Running;
            if (!save()) { job.status = ExportStatus::Pending; return {}; }
            auto request = job.request;
            request.job_id = job.id;
            request.overwrite = false;
            return request;
        }
        return {};
    }
    QString runningId() const {
        for (const auto& job : jobs) if (job.status == ExportStatus::Running) return job.id;
        return {};
    }
    bool hasPending() const {
        for (const auto& job : jobs)
            if (job.status == ExportStatus::Pending || job.status == ExportStatus::Running) return true;
        return false;
    }
    bool finish(const ExportProgress& state) {
        if (state.jobId.isEmpty() || state.jobId != runningId() || state.running ||
            (!state.hasResult && !state.error && !state.cancelled)) return false;
        for (auto& job : jobs) {
            if (job.id != state.jobId) continue;
            job.status = state.error ? ExportStatus::Failed : state.cancelled ? ExportStatus::Cancelled : ExportStatus::Done;
            job.error = state.error;
            return save();
        }
        return false;
    }
    bool retry(const QString& id, const QString& destination = {}) {
        for (auto& job : jobs) {
            if (job.id != id || (job.status != ExportStatus::Failed && job.status != ExportStatus::Cancelled && job.status != ExportStatus::Interrupted)) continue;
            if (!destination.isEmpty()) {
                auto options = job.request.options.value_or(ExportOptions{});
                options.path = destination;
                job.request.options = options;
            }
            job.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
            job.status = ExportStatus::Pending;
            job.error.reset();
            return save();
        }
        return false;
    }
    bool cancelPending() {
        paused = true;
        for (auto& job : jobs) if (job.status == ExportStatus::Pending) job.status = ExportStatus::Cancelled;
        return save();
    }
    bool clearFinished() {
        jobs.removeIf([](const ExportJob& job) { return job.status == ExportStatus::Done || job.status == ExportStatus::Cancelled; });
        return save();
    }
  private:
    QString path_;
    std::unique_ptr<QLockFile> lock_;
    bool lock() {
        if (!error.isEmpty())
            return false;
        if (lock_)
            return true;
        if (!QDir().mkpath(QFileInfo(path_).absolutePath()))
            return fail("Cannot create export queue directory.");
        auto guard = std::make_unique<QLockFile>(path_ + ".lock");
        guard->setStaleLockTime(0);
        if (!guard->tryLock())
            return fail("Export queue is in use by another window or its lock cannot be written. "
                        "Close the other window and restart.");
        lock_ = std::move(guard);
        return true;
    }
    bool fail(const QString& message) {
        error = message;
        paused = true;
        return false;
    }
};
} // namespace aigle
