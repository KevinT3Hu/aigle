#pragma once
#include "geometry_types.hpp"
#include "agent_types.hpp"
#include "gui_types.hpp"
#include "mask_types.hpp"
#include "retouch_types.hpp"
#include <QJsonObject>
#include <QMap>
namespace aigle {
struct CameraInfo {
    QString make, model, lens;
    std::optional<double> iso, aperture, focalLength;
};
struct ImageLoad {
    enum class Status { Loading, Ready, Failed };
    Status status = Status::Loading;
    QString error;
    ImageInfo info;
    QSize sourceSize;
    CameraInfo camera;
};
struct ImportProgress {
    bool running = false;
    int files = 0, total = 0;
    std::optional<QString> error;
};
struct DuplicateResult {
    qint64 id = 0;
    bool running = true, cancelled = false;
    std::optional<QString> error;
    QList<QStringList> groups;
    QList<QPair<QString, QString>> failures;
};
struct HistoryEntry { QString label; bool applied = false; };
struct EditorState {
    bool open = false, canUndo = false, canRedo = false, dirty = false, retouchPreview = false;
    std::optional<qint64> copy;
    QString photoIdentity;
    quint64 document = 0, revision = 0;
    DisplayDetection display;
    ModelDiscovery modelDiscovery;
    AgentState agent;
    ExportProgress exporting;
    ImportProgress importing;
    DuplicateResult duplicates;
    std::optional<QSizeF> extent;
    ImageLoad load;
    // The editor schema defines this document; widgets edit arbitrary paths in it.
    std::optional<QJsonObject> params;
    GeometryParameters geometry;
    RetouchParameters retouch;
    QList<RetouchGuide> retouchGuides;
    QList<MaskGuide> maskGuides;
    QList<HistoryEntry> history;
    int position = 0;
    std::optional<Histogram> histogram;
    std::optional<int> rating;
    std::optional<QString> flag, label;
    std::optional<QStringList> tags;
    std::optional<QMap<QString, QString>> iptc;
    QStringList snapshots;
};
}
Q_DECLARE_METATYPE(aigle::EditorState)
