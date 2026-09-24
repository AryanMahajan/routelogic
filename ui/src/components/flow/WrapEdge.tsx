import { BaseEdge, getSmoothStepPath, type EdgeProps } from "@xyflow/react";

/**
 * An edge that runs back to the left — from the end of one band of a wrapped chain to the
 * start of the next. It goes out to the right, along the gap just above the card it enters,
 * and down into it, so the long stretch never crosses a row of cards the way a curve or a
 * plain step through the midpoint would.
 */
export function WrapEdge({
  sourceX,
  sourceY,
  sourcePosition,
  targetX,
  targetY,
  targetPosition,
  label,
  markerEnd,
  style,
  interactionWidth,
}: EdgeProps) {
  // Handles sit about 20px below a card's top; a gap between rows is at least 60px.
  const above = targetY - 48;
  const [path, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    borderRadius: 14,
    offset: 24,
    centerY: above > sourceY ? above : undefined,
  });
  return (
    <BaseEdge
      path={path}
      label={label}
      labelX={labelX}
      labelY={above > sourceY ? above : labelY}
      markerEnd={markerEnd}
      style={style}
      interactionWidth={interactionWidth}
    />
  );
}
