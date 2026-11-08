#include "reply_json.hpp"
#include "editor_state_json.hpp"
#include "gui_json.hpp"
#include "command_json.hpp"
#include "engine.hpp"
#include <QJsonArray>
#include <QJsonDocument>

namespace aigle {
namespace {
struct StringDeleter {
    void operator()(char* p) const noexcept { aigle_string_free(p); }
};
using String = std::unique_ptr<char, StringDeleter>;
QJsonObject decode(char* raw) {
    const String text(raw);
    if (!text)
        return {{"error", "Engine returned an empty response"}};
    QJsonParseError error;
    const auto doc = QJsonDocument::fromJson(QByteArray(text.get()), &error);
    if (error.error != QJsonParseError::NoError || !doc.isObject())
        return {{"error", "Invalid engine response"}};
    return doc.object();
}
} // namespace
void Engine::start(const EngineConfig& config) {
    if (aigle_abi_version() != 1) {
        emit error(tr("Incompatible engine ABI"));
        return;
    }
    char* message = nullptr;
    QJsonObject options{{"process", "Gui"},
        {"backend", config.backend == EngineConfig::Backend::Cpu ? "Cpu" : config.backend == EngineConfig::Backend::Gpu ? "Gpu" : "Auto"},
        {"deterministic", config.deterministic}, {"overview_long_edge", config.overviewLongEdge}};
    if (config.workerThreads) options["worker_threads"] = *config.workerThreads;
    if (config.memoryBudget) options["memory_budget"] = qint64(*config.memoryBudget);
    if (config.cacheDirectory) options["cache_dir"] = *config.cacheDirectory;
    const auto bytes = QJsonDocument(QJsonObject{{"data_dir", config.dataDirectory}, {"engine", options}}).toJson(QJsonDocument::Compact);
    editor_.reset(aigle_editor_new(bytes.constData(), &message));
    const auto failure = message ? decode(message) : QJsonObject{};
    if (!editor_) {
        emit error(failure.value("error").toString(tr("Cannot initialize engine")));
        return;
    }
    timer_ = new QTimer(this);
    timer_->setInterval(33);
    connect(timer_, &QTimer::timeout, this, &Engine::poll);
    timer_->start();
    emit ready();
}
QJsonObject Engine::call(const QJsonObject& request) {
    const auto bytes = QJsonDocument(request).toJson(QJsonDocument::Compact);
    return decode(aigle_command(editor_.get(), bytes.constData()));
}
void Engine::command(const EngineCommand& command) {
    dispatch(toJson(command));
}
void Engine::dispatch(const QJsonObject& request) {
    const auto cmd = request.value("cmd").toString();
    if (!editor_) {
        if (cmd == "agent_credential" || cmd == "agent_models" || cmd == "scopes" ||
            cmd.startsWith("retouch_"))
            emit reply(replyFromJson(cmd, QJsonObject{{"error", tr("Engine is not ready")},
                                        {"request_id", request.value("request_id")},
                                        {"session", request.value("session")}}));
        else
            emit error(tr("Engine is not ready"));
        return;
    }
    const auto result = call(request);
    if (result.contains("error")) {
        if (cmd == "agent_credential" || cmd == "agent_models" || cmd == "scopes" ||
            cmd.startsWith("retouch_")) {
            emit reply(replyFromJson(cmd, QJsonObject{{"error", result.value("error")},
                                        {"request_id", request.value("request_id")},
                                        {"session", request.value("session")}}));
        } else if (cmd == "file_action" || cmd == "find_duplicates" || cmd == "export" || cmd == "export_inputs") {
            auto failure = request;
            failure["ok"] = false;
            failure["error"] = result.value("error");
            emit reply(replyFromJson(cmd, failure));
            poll();
        } else
            emit error(result.value("error").toString());
        return;
    }
    if (cmd == "open" || cmd == "synthetic" || cmd == "remote_open" || cmd == "close" ||
        cmd == "remove_copy" || cmd == "file_action")
        revision_ = -1;
    auto value = result.value("ok");
    if (cmd == "agent_credential" || cmd == "agent_models" || cmd == "scopes" ||
        cmd.startsWith("retouch_")) {
        auto credential = value.toObject();
        credential.insert("request_id", request.value("request_id"));
        credential.insert("session", request.value("session"));
        value = credential;
    }
    if (cmd == "open") {
        auto opened = value.toObject();
        opened.insert("path", request.value("path"));
        opened.insert("copy", request.value("copy"));
        opened.insert("develop", request.value("develop").toBool(true));
        value = opened;
    }
    if (cmd == "albums")
        value = QJsonObject{{"albums", value},
                            {"action", request.value("action").toString("open")},
                            {"paths", request.value("paths")}};
    emit reply(replyFromJson(cmd == "scan" && request.value("preserve_mode").toBool() ? "scan_develop" : cmd,
               value));
    poll();
}
void Engine::poll() {
    if (!editor_)
        return;
    auto result = call({{"cmd", "state"}, {"revision", revision_}});
    if (result.contains("error")) {
        emit error(result.value("error").toString());
        return;
    }
    const auto state = result.value("ok").toObject();
    // Fetch params again when decoding completes so persisted mask guides are projected.
    revision_ = state.value("load").toObject().contains("Ready")
                    ? state.value("revision").toInteger(-1)
                    : -1;
    emit stateChanged(editorStateFromJson(state));
    for (const bool original : {false, true}) {
        if (auto* raw = aigle_frame_take(editor_.get(), original ? 1U : 0U))
            emit frameReady(Frame(raw, FrameDeleter{}), original);
    }
    if (!comparisonSession_.isEmpty()) {
        emit comparisonDisplayReady(comparisonSession_, displayDetectionFromJson(call({{"cmd", "comparison_display_state"}}).value("ok").toObject()));
        const auto states = call({{"cmd", "comparison_state"}}).value("ok").toArray();
        for (int i = 0; i < states.size(); ++i) {
            const auto comparisonState = states[i].toObject();
            const auto extent = comparisonState["extent"].toArray();
            QSizeF size;
            if (extent.size() == 2)
                size = {extent[0].toDouble(), extent[1].toDouble()};
            QImage image;
            QRectF region;
            const Frame frame(aigle_comparison_frame_take(editor_.get(), unsigned(i)),
                              FrameDeleter{});
            if (frame) {
                emit comparisonFrameReady(comparisonSession_, i, frame);
                const auto p = aigle_frame_pixels(frame.get());
                if (p.format == 0)
                    image =
                        QImage(p.data, int(p.width), int(p.height), p.stride, QImage::Format_ARGB32)
                            .copy();
                region = {p.x, p.y, p.w, p.h};
            }
            emit comparisonFrame(comparisonSession_, i, image, region, size,
                                 comparisonState["load"].toObject()["Failed"].toString());
        }
    }
    if (!thumbnails_.isEmpty()) {
        const auto path = thumbnails_.dequeue();
        const auto metadata = call({{"cmd", "library_metadata"}, {"path", path}});
        if (metadata.contains("ok"))
            emit reply(replyFromJson("library_metadata", metadata.value("ok")));
        const auto bytes = path.toUtf8();
        char* message = nullptr;
        const Frame frame(aigle_thumbnail(editor_.get(), bytes.constData(), 512, &message),
                          FrameDeleter{});
        const String failure(message);
        if (frame) {
            const auto pixels = aigle_frame_pixels(frame.get());
            const QImage image(pixels.data, int(pixels.width), int(pixels.height), pixels.stride,
                               QImage::Format_ARGB32);
            emit thumbnailReady(path, image.copy());
        }
    }
}
void Engine::thumbnails(const QStringList& paths) {
    thumbnails_.clear();
    for (const auto& path : paths)
        thumbnails_.enqueue(path);
}
void Engine::comparisonStart(const QList<PhotoIdentity>& paths, const QString& session) {
    comparisonSession_.clear();
    const auto result = call({{"cmd", "comparison_open"}, {"targets", toJson(paths)}});
    if (result.contains("error")) {
        for (int i = 0; i < paths.size(); ++i)
            emit comparisonFrame(session, i, {}, {}, {}, result["error"].toString());
        return;
    }
    comparisonSession_ = session;
}
void Engine::comparisonDisplay(const QString& session, const QString& screen) {
    if (comparisonSession_ != session) return;
    (void)call({{"cmd", "comparison_detect_display"}, {"screen", screen}});
}
void Engine::comparisonStop(const QString& session) {
    if (comparisonSession_ != session)
        return;
    comparisonSession_.clear();
    (void)call({{"cmd", "comparison_close"}});
}
void Engine::comparisonViewport(const QString& session, int index, const ViewportRequest& viewport) {
    if (comparisonSession_ != session)
        return;
    const auto result =
        call({{"cmd", "comparison_viewport"}, {"index", index}, {"viewport", viewportJson(viewport)}});
    if (result.contains("error"))
        emit comparisonFrame(session, index, {}, {}, {}, result["error"].toString());
}
void Engine::comparisonPreview(const QString& path, const QString& session) {
    if (!editor_) {
        emit comparisonReady(session, path, {}, tr("Engine is not ready"));
        return;
    }
    char* message = nullptr;
    const auto bytes = path.toUtf8();
    const Frame frame(aigle_comparison_preview(editor_.get(), bytes.constData(), 2048, &message),
                      FrameDeleter{});
    const auto failure = message ? decode(message).value("error").toString() : QString{};
    QImage image;
    if (frame) {
        const auto pixels = aigle_frame_pixels(frame.get());
        image = QImage(pixels.data, int(pixels.width), int(pixels.height), pixels.stride,
                       QImage::Format_ARGB32)
                    .copy();
    }
    emit comparisonReady(session, path, image, failure);
}
void Engine::stop() {
    if (timer_)
        timer_->stop();
    editor_.reset();
}
} // namespace aigle

void aigle::Engine::setViewport(const aigle::ViewportRequest& viewport) { dispatch(toJson(viewport)); }
