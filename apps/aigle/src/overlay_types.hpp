#pragma once
#include <QString>
#include <optional>
namespace aigle {
// An engaged outer optional applies a patch; an empty inner optional clears it.
struct OverlayPatch {
    std::optional<bool> clipping = std::nullopt, focusPeaking = std::nullopt, gamutWarning = std::nullopt;
    std::optional<std::optional<qint64>> mask = std::nullopt;
    std::optional<std::optional<QString>> proof = std::nullopt;
    void apply(const OverlayPatch& value) {
        if (value.clipping) clipping = value.clipping;
        if (value.focusPeaking) focusPeaking = value.focusPeaking;
        if (value.gamutWarning) gamutWarning = value.gamutWarning;
        if (value.mask) mask = value.mask;
        if (value.proof) proof = value.proof;
    }
};
}
