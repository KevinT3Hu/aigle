#pragma once
#include <QRectF>
namespace aigle {
struct Orientation {
    int quarterTurns = 0;
    bool flip = false;
};
struct CropAspect {
    enum class Kind { Free, Original, Ratio };
    Kind kind = Kind::Free;
    double width = 1, height = 1;
};
struct GeometryParameters {
    QRectF crop{0, 0, 1, 1};
    Orientation orientation;
    CropAspect aspect;
    double angle = 0;
};
}
