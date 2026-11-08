#pragma once
#include "photo_types.hpp"
#include "edit_operation.hpp"
#include "export_options.hpp"
#include "geometry_types.hpp"
#include "overlay_types.hpp"
#include "retouch_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
#include <QMetaType>
#include <QRectF>
#include <optional>
#include <variant>

namespace aigle::commands {
struct Retouch {
    enum class Phase { Commit, Preview, Placement };
    Phase phase = Phase::Commit;
    std::optional<QString> action = std::nullopt;
    std::optional<qint64> document_id = std::nullopt;
    std::optional<qint64> revision = std::nullopt;
    std::optional<QString> session = std::nullopt;
    std::optional<QString> kind = std::nullopt;
    std::optional<qint64> index = std::nullopt;
    std::optional<RetouchCorrection> expected = std::nullopt;
    std::optional<QString> label = std::nullopt;
    std::optional<QString> field = std::nullopt;
    std::optional<QJsonValue> value = std::nullopt;
    std::optional<QString> handle = std::nullopt;
    std::optional<QList<QPointF>> points = std::nullopt;
    std::optional<bool> all = std::nullopt;
    std::optional<QString> tool = std::nullopt;
    std::optional<double> radius = std::nullopt;
    std::optional<double> feather = std::nullopt;
    std::optional<double> opacity = std::nullopt;
    std::optional<double> pupil = std::nullopt;
    std::optional<double> darken = std::nullopt;
};

// Commands have named fields and a closed set of alternatives. JSON-valued
// members describe schema-defined edits or wire-format documents, not envelopes.
struct AgentAccept {
};
struct AgentCancel {
};
struct AgentCredential {
    std::optional<QString> request_id = std::nullopt;
    std::optional<QString> provider_id = std::nullopt;
    std::optional<QString> endpoint = std::nullopt;
    std::optional<QString> action = std::nullopt;
    std::optional<QString> api_key = std::nullopt;
};
struct AgentModels {
    std::optional<QString> request_id = std::nullopt;
    std::optional<QString> provider_id = std::nullopt;
    std::optional<QString> endpoint = std::nullopt;
    std::optional<QString> api_key = std::nullopt;
};
struct AgentModelsCancel {
    std::optional<QString> request_id = std::nullopt;
};
struct AgentReject {
};
struct AgentStart {
    std::optional<QString> provider_id = std::nullopt;
    std::optional<QString> endpoint = std::nullopt;
    std::optional<QString> model = std::nullopt;
    std::optional<QString> api_key = std::nullopt;
    std::optional<QString> prompt = std::nullopt;
};
struct AlbumPaths {
    std::optional<QJsonValue> id = std::nullopt;
};
struct Albums {
    std::optional<QString> action = std::nullopt;
    std::optional<QStringList> paths = std::nullopt;
};
struct AutoTone {
};
struct AutoWb {
};
struct BatchMetadata {
    std::optional<QList<PhotoIdentity>> targets = std::nullopt;
    std::optional<QJsonValue> stack = std::nullopt;
    std::optional<QString> flag = std::nullopt;
    std::optional<qint64> rating = std::nullopt;
    std::optional<QJsonValue> label = std::nullopt;
    std::optional<QStringList> tags = std::nullopt;
    std::optional<QStringList> remove_tags = std::nullopt;
    std::optional<QString> tag_mode = std::nullopt;
    std::optional<QMap<QString, QString>> iptc = std::nullopt;
};
struct CancelDuplicates {
};
struct CancelExport {
};
struct CancelImport {
};
struct CatalogBackup {
    std::optional<QString> path = std::nullopt;
};
struct CatalogRestore {
    std::optional<QString> path = std::nullopt;
};
struct Copies {
};
struct CreateAlbum {
    std::optional<QString> name = std::nullopt;
    std::optional<QStringList> paths = std::nullopt;
};
struct CreateCopies {
    std::optional<QString> name = std::nullopt;
    std::optional<QList<PhotoIdentity>> targets = std::nullopt;
};
struct CreateCopy {
    std::optional<QString> name = std::nullopt;
};
struct CropOrientation {
    std::optional<Orientation> orientation = std::nullopt;
};
struct DeleteAlbum {
    std::optional<QJsonValue> id = std::nullopt;
};
struct DetectDisplay {
    std::optional<QString> screen = std::nullopt;
};
struct Edit {
    std::optional<QString> label = std::nullopt;
    std::optional<QList<EditOperation>> ops = std::nullopt;
    std::optional<qint64> gesture = std::nullopt;
    std::optional<bool> continuous = std::nullopt;
};
struct Export {
    bool operator==(const Export&) const = default;
    std::optional<ExportOptions> options = std::nullopt;
    std::optional<bool> overwrite = std::nullopt;
    std::optional<QJsonValue> source_ref = std::nullopt;
    std::optional<QJsonObject> params = std::nullopt;
    std::optional<QString> source_name = std::nullopt;
    std::optional<QString> job_id = std::nullopt;
    std::optional<bool> paired_sdr = std::nullopt;
};
struct ExportInputs {
    std::optional<bool> preview = std::nullopt;
    std::optional<bool> current = std::nullopt;
    std::optional<QList<PhotoIdentity>> targets = std::nullopt;
};
struct FileAction {
    std::optional<QString> action = std::nullopt;
    std::optional<QString> path = std::nullopt;
    std::optional<QJsonValue> copy = std::nullopt;
    std::optional<QString> destination = std::nullopt;
};
struct FindDuplicates {
    std::optional<QStringList> paths = std::nullopt;
    std::optional<QJsonValue> id = std::nullopt;
};
struct Goto {
    std::optional<qint64> position = std::nullopt;
};
struct Import {
    std::optional<QString> source = std::nullopt;
    std::optional<QString> destination = std::nullopt;
    std::optional<QString> folders = std::nullopt;
    std::optional<QString> names = std::nullopt;
    std::optional<QString> backup = std::nullopt;
};
struct ImportProfile {
    std::optional<QString> kind = std::nullopt;
    std::optional<QString> path = std::nullopt;
};
struct LibraryCopies {
    std::optional<QStringList> paths = std::nullopt;
};
struct LibraryMetadata {
    std::optional<QString> path = std::nullopt;
    std::optional<QJsonValue> copy = std::nullopt;
};
struct MatchLens {
};
struct Metadata {
    std::optional<qint64> rating = std::nullopt;
    std::optional<QStringList> tags = std::nullopt;
    std::optional<QMap<QString, QString>> iptc = std::nullopt;
    std::optional<QString> flag = std::nullopt;
    std::optional<QJsonValue> label = std::nullopt;
    std::optional<QJsonValue> stack = std::nullopt;
    std::optional<QStringList> remove_tags = std::nullopt;
    std::optional<QString> tag_mode = std::nullopt;
};
struct Open {
    std::optional<QString> path = std::nullopt;
    std::optional<bool> develop = std::nullopt;
    std::optional<QJsonValue> copy = std::nullopt;
};
struct Overlays {
    std::optional<OverlayPatch> overlays = std::nullopt;
};
struct PasteSelected {
    std::optional<QJsonObject> params = std::nullopt;
    std::optional<QStringList> groups = std::nullopt;
};
struct Redo {
};
struct RemoteOpen {
    std::optional<QString> server = std::nullopt;
    std::optional<QJsonValue> id = std::nullopt;
};
struct RemoveCopy {
    std::optional<QJsonValue> id = std::nullopt;
};
struct RenameAlbum {
    std::optional<QJsonValue> id = std::nullopt;
    std::optional<QString> name = std::nullopt;
};
struct RenameCopy {
    std::optional<QJsonValue> id = std::nullopt;
    std::optional<QString> name = std::nullopt;
};
struct RestoreSnapshot {
    std::optional<qint64> index = std::nullopt;
};
struct RetouchCancel {
    std::optional<QString> session = std::nullopt;
};



struct Save {
};
struct Scan {
    std::optional<QString> path = std::nullopt;
    std::optional<bool> recursive = std::nullopt;
    std::optional<bool> preserve_mode = std::nullopt;
};
struct Schema {
};
struct Scopes {
    std::optional<QString> request_id = std::nullopt;
    std::optional<qint64> target = std::nullopt;
    std::optional<QRectF> roi = std::nullopt;
};
struct Snapshot {
    std::optional<QString> name = std::nullopt;
};
struct SyncSelected {
    std::optional<QJsonObject> params = std::nullopt;
    std::optional<QStringList> groups = std::nullopt;
    std::optional<QList<PhotoIdentity>> targets = std::nullopt;
};
struct Synthetic {
};
struct Tool {
    std::optional<QString> tool = std::nullopt;
    std::optional<double> angle = std::nullopt;
    std::optional<CropAspect> aspect = std::nullopt;
    std::optional<Orientation> orientation = std::nullopt;
    std::optional<bool> normalized = std::nullopt;
    std::optional<QList<QPointF>> points = std::nullopt;
    std::optional<QJsonValue> id = std::nullopt;
    std::optional<QString> handle = std::nullopt;
    std::optional<double> radius = std::nullopt;
    std::optional<qint64> mask_id = std::nullopt;
    std::optional<double> feather = std::nullopt;
    std::optional<double> opacity = std::nullopt;
    std::optional<double> pupil = std::nullopt;
    std::optional<double> darken = std::nullopt;
    std::optional<qint64> document_id = std::nullopt;
    std::optional<qint64> revision = std::nullopt;
};
struct ToolMode {
    std::optional<QString> tool = std::nullopt;
};
struct Undo {
};
struct UpdateAlbum {
    std::optional<QJsonValue> id = std::nullopt;
    std::optional<QStringList> paths = std::nullopt;
    std::optional<bool> add = std::nullopt;
};
}
namespace aigle {
using EngineCommand = std::variant<
    commands::AgentAccept,
    commands::AgentCancel,
    commands::AgentCredential,
    commands::AgentModels,
    commands::AgentModelsCancel,
    commands::AgentReject,
    commands::AgentStart,
    commands::AlbumPaths,
    commands::Albums,
    commands::AutoTone,
    commands::AutoWb,
    commands::BatchMetadata,
    commands::CancelDuplicates,
    commands::CancelExport,
    commands::CancelImport,
    commands::CatalogBackup,
    commands::CatalogRestore,
    commands::Copies,
    commands::CreateAlbum,
    commands::CreateCopies,
    commands::CreateCopy,
    commands::CropOrientation,
    commands::DeleteAlbum,
    commands::DetectDisplay,
    commands::Edit,
    commands::Export,
    commands::ExportInputs,
    commands::FileAction,
    commands::FindDuplicates,
    commands::Goto,
    commands::Import,
    commands::ImportProfile,
    commands::LibraryCopies,
    commands::LibraryMetadata,
    commands::MatchLens,
    commands::Metadata,
    commands::Open,
    commands::Overlays,
    commands::PasteSelected,
    commands::Redo,
    commands::RemoteOpen,
    commands::RemoveCopy,
    commands::RenameAlbum,
    commands::RenameCopy,
    commands::RestoreSnapshot,
    commands::RetouchCancel,
    commands::Retouch,
    commands::Save,
    commands::Scan,
    commands::Schema,
    commands::Scopes,
    commands::Snapshot,
    commands::SyncSelected,
    commands::Synthetic,
    commands::Tool,
    commands::ToolMode,
    commands::Undo,
    commands::UpdateAlbum>;
}
Q_DECLARE_METATYPE(aigle::EngineCommand)
