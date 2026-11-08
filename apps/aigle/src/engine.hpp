#pragma once
#include "engine_config.hpp"
#include "replies.hpp"
#include "editor_state.hpp"
#include "commands.hpp"
#include "gui_types.hpp"
#include "aigle.h"
#include <QImage>
#include <QJsonObject>
#include <QJsonArray>
#include <QObject>
#include <QQueue>
#include <QTimer>
#include <memory>

namespace aigle {
struct EditorDeleter {
    void operator()(AigleEditor* p) const noexcept { aigle_editor_free(p); }
};
struct FrameDeleter {
    void operator()(AigleFrame* p) const noexcept { aigle_frame_free(p); }
};
using Frame = std::shared_ptr<AigleFrame>;
// This object and the Rust editor live exclusively on the engine thread. Only
// value types and independently retained immutable frames cross into the GUI.
class Engine final : public QObject {
    Q_OBJECT
  public:
    explicit Engine(QObject* parent = nullptr) : QObject(parent) {}
  public slots:
    void start(const aigle::EngineConfig& config);
    void command(const aigle::EngineCommand& request);
    void setViewport(const aigle::ViewportRequest& viewport);
    void stop();
    void thumbnails(const QStringList& paths);
    void comparisonStart(const QList<PhotoIdentity>& paths, const QString& session);
    void comparisonDisplay(const QString& session, const QString& screen);
    void comparisonStop(const QString& session);
    void comparisonViewport(const QString& session, int index, const aigle::ViewportRequest& viewport);
    void comparisonPreview(const QString& path, const QString& session);
  signals:
    void ready();
    void reply(const aigle::EngineReply& result);
    void stateChanged(const aigle::EditorState& state);
    void frameReady(aigle::Frame frame, bool original);
    void thumbnailReady(const QString& path, const QImage& image);
    void error(const QString& message);
    void comparisonDisplayReady(const QString& session, const aigle::DisplayDetection& profile);
    void comparisonFrameReady(const QString& session, int index, aigle::Frame frame);
    void comparisonFrame(const QString& session, int index, const QImage& image, const QRectF& region, const QSizeF& extent, const QString& error);
    void comparisonReady(const QString& session, const QString& path, const QImage& image, const QString& error);

  private:
    void poll();
    void dispatch(const QJsonObject& request);
    [[nodiscard]] QJsonObject call(const QJsonObject& request);
    std::unique_ptr<AigleEditor, EditorDeleter> editor_;
    QTimer* timer_ = nullptr;
    qint64 revision_ = -1;
    QQueue<QString> thumbnails_;
    QString comparisonSession_;
};
} // namespace aigle
Q_DECLARE_METATYPE(aigle::Frame)
